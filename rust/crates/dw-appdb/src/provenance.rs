//! Positive provenance (DEC-125, DASHPAY §2.2 rule 5): the Platform entities
//! whose latest fetch verified against SPV-held quorum keys. Absence means
//! unverified once the trusted-quorum fallback has been used; the engine
//! decides that, this module only keeps the records.

use rusqlite::params;

use crate::{AppDb, Result};

impl AppDb {
    /// Records that `(kind, key)` verified at `now` (a later verification
    /// moves `verified_at` forward).
    pub fn mark_verified(&self, kind: &str, key: &str, now: u64) -> Result<()> {
        self.conn().execute(
            "INSERT INTO dp_trust_verified (kind, key, verified_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (kind, key) DO UPDATE
                 SET verified_at = MAX(verified_at, excluded.verified_at)",
            params![kind, key, now as i64],
        )?;
        Ok(())
    }

    /// Whether `(kind, key)` has a verification record.
    pub fn is_verified(&self, kind: &str, key: &str) -> Result<bool> {
        Ok(self
            .conn()
            .prepare_cached("SELECT 1 FROM dp_trust_verified WHERE kind = ?1 AND key = ?2")?
            .exists(params![kind, key])?)
    }

    /// Every `(kind, key)` with a verification record.
    pub fn verified_entities(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT kind, key FROM dp_trust_verified")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verification_record_is_global_and_keeps_its_latest_time() {
        let db = AppDb::open_in_memory().unwrap();
        assert!(!db.is_verified("identity", "x").unwrap());
        db.mark_verified("identity", "x", 9).unwrap();
        db.mark_verified("identity", "x", 5).unwrap();
        assert!(db.is_verified("identity", "x").unwrap());
        assert!(!db.is_verified("profile", "x").unwrap());
        assert_eq!(
            db.verified_entities().unwrap(),
            vec![("identity".to_string(), "x".to_string())]
        );
        // Wallet removal leaves it: verification is about Platform data.
        db.delete_wallet("aa").unwrap();
        let at: i64 = db
            .conn()
            .query_row(
                "SELECT verified_at FROM dp_trust_verified WHERE kind = 'identity'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(at, 9);
    }
}
