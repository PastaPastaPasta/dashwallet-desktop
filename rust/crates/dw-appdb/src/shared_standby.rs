//! The single writer of `shared_mn_standby` (MG-04b §9.1, §9.4): a
//! wallet's standbys, each a pair of signed unilateral dissolutions by one
//! share paying one fee. A row is added, or a variant it lacks is added to
//! it; nothing replaces a stored variant. Every write is synced
//! ([`AppDb::durably`]): `create_standby` returns only what is stored.

use rusqlite::{OptionalExtension, Row, params};

use crate::{AppDb, Result};

/// One `shared_mn_standby` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandbyRow {
    /// 64 lower-case hex, as displayed.
    pub pro_tx_hash: String,
    pub share_index: u16,
    pub fee: u64,
    /// The full-principal variant's consensus bytes.
    pub full_tx: Option<Vec<u8>>,
    /// The immediate variant's; `None` at penalty 0 (m-20).
    pub immediate_tx: Option<Vec<u8>>,
    pub full_valid_from: u32,
    /// `created` or `imported`.
    pub source: String,
    pub created_at: u64,
}

const COLUMNS: &str = "pro_tx_hash, share_index, fee, full_tx, immediate_tx, full_valid_from, \
     source, created_at";

fn read_row(r: &Row<'_>) -> rusqlite::Result<StandbyRow> {
    Ok(StandbyRow {
        pro_tx_hash: r.get(0)?,
        share_index: r.get(1)?,
        fee: u64::try_from(r.get::<_, i64>(2)?).unwrap_or(0),
        full_tx: r.get(3)?,
        immediate_tx: r.get(4)?,
        full_valid_from: r.get(5)?,
        source: r.get(6)?,
        created_at: u64::try_from(r.get::<_, i64>(7)?).unwrap_or(0),
    })
}

fn sql_u64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

impl AppDb {
    /// Stores `row`, synced: a new standby, or the variants a stored one of
    /// the same share and fee lacks (the second line of a file, say). A
    /// stored variant, its source and its date are kept. `true` when
    /// anything was written.
    pub fn store_standby_durable(&self, wallet_id: &str, row: &StandbyRow) -> Result<bool> {
        self.durably(|conn| {
            Ok(conn.execute(
                &format!(
                    "INSERT INTO shared_mn_standby (wallet_id, {COLUMNS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT (wallet_id, pro_tx_hash, share_index, fee) DO UPDATE SET
                         full_tx = COALESCE(full_tx, excluded.full_tx),
                         immediate_tx = COALESCE(immediate_tx, excluded.immediate_tx)
                     WHERE (full_tx IS NULL AND excluded.full_tx IS NOT NULL)
                        OR (immediate_tx IS NULL AND excluded.immediate_tx IS NOT NULL)"
                ),
                params![
                    wallet_id,
                    row.pro_tx_hash,
                    row.share_index,
                    sql_u64(row.fee),
                    row.full_tx,
                    row.immediate_tx,
                    row.full_valid_from,
                    row.source,
                    sql_u64(row.created_at),
                ],
            )? == 1)
        })
    }

    /// The wallet's standbys of `pro_tx_hash` (every masternode's when
    /// `None`), by masternode, share and fee.
    pub fn standbys(&self, wallet_id: &str, pro_tx_hash: Option<&str>) -> Result<Vec<StandbyRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM shared_mn_standby
             WHERE wallet_id = ?1 AND (?2 IS NULL OR pro_tx_hash = ?2)
             ORDER BY pro_tx_hash, share_index, fee"
        ))?;
        let rows = stmt.query_map(params![wallet_id, pro_tx_hash], read_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn standby(
        &self,
        wallet_id: &str,
        pro_tx_hash: &str,
        share_index: u16,
        fee: u64,
    ) -> Result<Option<StandbyRow>> {
        let conn = self.conn();
        Ok(conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM shared_mn_standby
                     WHERE wallet_id = ?1 AND pro_tx_hash = ?2 AND share_index = ?3 AND fee = ?4"
                ),
                params![wallet_id, pro_tx_hash, share_index, sql_u64(fee)],
                read_row,
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: &str = "aa";
    const W2: &str = "bb";

    fn hash(n: u8) -> String {
        format!("{n:02x}").repeat(32)
    }

    fn row(n: u8, share: u16, fee: u64, full: Option<u8>, immediate: Option<u8>) -> StandbyRow {
        StandbyRow {
            pro_tx_hash: hash(n),
            share_index: share,
            fee,
            full_tx: full.map(|b| vec![b; 4]),
            immediate_tx: immediate.map(|b| vec![b; 4]),
            full_valid_from: 150,
            source: "created".into(),
            created_at: 10,
        }
    }

    #[test]
    fn a_standby_is_added_then_only_completed() {
        let db = AppDb::open_in_memory().unwrap();
        let both = row(1, 0, 1_000, Some(1), Some(2));
        assert!(db.store_standby_durable(W, &both).unwrap());
        // The same again, or other bytes for stored variants: kept as is.
        assert!(!db.store_standby_durable(W, &both).unwrap());
        let other = StandbyRow {
            source: "imported".into(),
            created_at: 99,
            ..row(1, 0, 1_000, Some(7), Some(8))
        };
        assert!(!db.store_standby_durable(W, &other).unwrap());
        assert_eq!(db.standby(W, &hash(1), 0, 1_000).unwrap(), Some(both));

        // A file's lines one at a time: the second completes the row.
        let imported = |full, immediate| StandbyRow {
            source: "imported".into(),
            ..row(2, 1, 5_000, full, immediate)
        };
        assert!(
            db.store_standby_durable(W, &imported(None, Some(4)))
                .unwrap()
        );
        assert!(
            db.store_standby_durable(W, &imported(Some(3), None))
                .unwrap()
        );
        assert!(
            !db.store_standby_durable(W, &imported(Some(3), None))
                .unwrap()
        );
        assert_eq!(
            db.standby(W, &hash(2), 1, 5_000).unwrap(),
            Some(imported(Some(3), Some(4)))
        );
        // Another fee is another standby; another wallet sees none.
        assert!(
            db.store_standby_durable(W, &row(1, 0, 2_000, Some(5), None))
                .unwrap()
        );
        let fees: Vec<_> = db
            .standbys(W, Some(&hash(1)))
            .unwrap()
            .iter()
            .map(|r| r.fee)
            .collect();
        assert_eq!(fees, [1_000, 2_000]);
        assert_eq!(db.standbys(W, None).unwrap().len(), 3);
        assert!(db.standbys(W2, None).unwrap().is_empty());
        assert_eq!(db.standby(W, &hash(1), 1, 1_000).unwrap(), None);
    }

    #[test]
    fn a_standby_holds_a_variant_and_a_valid_key() {
        let db = AppDb::open_in_memory().unwrap();
        assert!(
            db.store_standby_durable(W, &row(1, 0, 1_000, None, None))
                .is_err()
        );
        assert!(
            db.store_standby_durable(W, &row(1, 0, 0, Some(1), None))
                .is_err()
        );
        assert!(
            db.store_standby_durable(W, &row(1, 8, 1_000, Some(1), None))
                .is_err()
        );
        let upper = StandbyRow {
            pro_tx_hash: "AA".repeat(32),
            ..row(1, 0, 1_000, Some(1), None)
        };
        assert!(db.store_standby_durable(W, &upper).is_err());
        let source = StandbyRow {
            source: "other".into(),
            ..row(1, 0, 1_000, Some(1), None)
        };
        assert!(db.store_standby_durable(W, &source).is_err());
        assert!(db.standbys(W, None).unwrap().is_empty());
    }

    #[test]
    fn a_removed_wallets_standby_goes_and_is_not_written() {
        let db = AppDb::open_in_memory().unwrap();
        db.store_standby_durable(W, &row(1, 0, 1_000, Some(1), None))
            .unwrap();
        db.store_standby_durable(W2, &row(1, 0, 1_000, Some(1), None))
            .unwrap();
        db.delete_wallet(W).unwrap();
        assert!(db.standbys(W, None).unwrap().is_empty());
        assert_eq!(db.standbys(W2, None).unwrap().len(), 1);
        assert!(
            !db.store_standby_durable(W, &row(2, 0, 1_000, Some(1), None))
                .unwrap()
        );
        assert!(db.standbys(W, None).unwrap().is_empty());
    }
}
