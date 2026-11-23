//! Positive provenance (DEC-125, DASHPAY §2.2 rule 5): the Platform entities
//! whose latest fetch verified against SPV-held quorum keys. Absence means
//! unverified once the trusted-quorum fallback has been used; the engine
//! decides that, this module only keeps the records.

use rusqlite::params;

use crate::{AppDb, Result};

impl AppDb {
    /// Records that `(kind, key)` verified at `now` with content `digest`
    /// (DEC-183). The same digest again moves `verified_at` forward; a new
    /// digest replaces the old one and its time.
    pub fn mark_verified(&self, kind: &str, key: &str, digest: &str, now: u64) -> Result<()> {
        self.conn().execute(
            "INSERT INTO dp_trust_verified (kind, key, verified_at, digest) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (kind, key) DO UPDATE
                 SET verified_at = CASE WHEN digest IS excluded.digest
                                        THEN MAX(verified_at, excluded.verified_at)
                                        ELSE excluded.verified_at END,
                     digest = excluded.digest",
            params![kind, key, now as i64, digest],
        )?;
        Ok(())
    }

    /// Every `(kind, key, digest)` with a verification record. Rows without
    /// a digest (written before DEC-183) are left out: they verify nothing.
    pub fn verified_entities(&self) -> Result<Vec<(String, String, String)>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT kind, key, digest FROM dp_trust_verified WHERE digest IS NOT NULL")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entities(db: &AppDb) -> Vec<(String, String, String)> {
        db.verified_entities().unwrap()
    }

    fn row(kind: &str, key: &str, digest: &str) -> (String, String, String) {
        (kind.into(), key.into(), digest.into())
    }

    fn verified_at(db: &AppDb) -> i64 {
        db.conn()
            .query_row(
                "SELECT verified_at FROM dp_trust_verified WHERE kind = 'identity'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn a_verification_record_is_global_and_keeps_its_latest_time() {
        let db = AppDb::open_in_memory().unwrap();
        assert!(entities(&db).is_empty());
        db.mark_verified("identity", "x", "v1:aa", 9).unwrap();
        db.mark_verified("identity", "x", "v1:aa", 5).unwrap();
        assert_eq!(entities(&db), vec![row("identity", "x", "v1:aa")]);
        // Wallet removal leaves it: verification is about Platform data.
        db.delete_wallet("aa").unwrap();
        assert_eq!(verified_at(&db), 9);
    }

    /// DEC-183: a record is about one content; new content replaces it.
    #[test]
    fn a_new_digest_replaces_the_record_and_its_time() {
        let db = AppDb::open_in_memory().unwrap();
        db.mark_verified("identity", "x", "v1:aa", 9).unwrap();
        db.mark_verified("identity", "x", "v1:bb", 5).unwrap();
        assert_eq!(entities(&db), vec![row("identity", "x", "v1:bb")]);
        assert_eq!(verified_at(&db), 5);
    }
}
