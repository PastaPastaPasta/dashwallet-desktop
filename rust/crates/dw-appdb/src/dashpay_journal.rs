//! What the changeset tap (DASHPAY §3.5, ROADMAP E0-06) writes to the
//! `dp_events` journal and `dp_trust_unverified`, and the rows it reads to
//! classify a changeset.

use rusqlite::{OptionalExtension, params};

use crate::{AppDb, Result};

/// One `dp_events` row to write. `contact` and `reference` are `""` when the
/// event has none (the table's convention).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub identity: String,
    /// The `EventKind` name, snake case (`payment_received`).
    pub kind: &'static str,
    pub contact: String,
    pub reference: String,
    /// `Some` stores the event as read (catch-up silence, §2.7).
    pub read_at: Option<u64>,
    /// Skip the event when the identity already has a row of one of these
    /// kinds for the same contact (once-per-relationship events).
    pub unless_any: &'static [&'static str],
}

/// One `dp_events` row as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRow {
    pub id: i64,
    pub identity: String,
    pub kind: String,
    pub contact: String,
    pub reference: String,
    pub at: u64,
    pub read_at: Option<u64>,
}

/// An entity seen through the trusted-quorum fallback (§2.2):
/// `kind` is `identity`, `contact_request` or `dpns_label`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnverifiedEntity {
    pub kind: &'static str,
    pub key: String,
}

impl AppDb {
    /// Writes one changeset's journal events in one transaction, `INSERT OR
    /// IGNORE`: an event already journaled keeps its row (and its read
    /// state). Returns how many events were new.
    pub fn record_events(
        &self,
        wallet_id: &str,
        events: &[JournalEntry],
        now: u64,
    ) -> Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut added = 0;
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR IGNORE INTO dp_events
                     (wallet_id, identity, kind, contact, ref, at, read_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            let mut exists = tx.prepare_cached(
                "SELECT 1 FROM dp_events
                 WHERE wallet_id = ?1 AND identity = ?2 AND contact = ?3 AND kind = ?4",
            )?;
            for e in events {
                let mut seen = false;
                for kind in e.unless_any {
                    if exists.exists(params![wallet_id, e.identity, e.contact, kind])? {
                        seen = true;
                        break;
                    }
                }
                if seen {
                    continue;
                }
                added += insert.execute(params![
                    wallet_id,
                    e.identity,
                    e.kind,
                    e.contact,
                    e.reference,
                    now as i64,
                    e.read_at.map(|t| t as i64),
                ])?;
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// Flags entities seen through the trusted-quorum fallback, in one
    /// transaction, `INSERT OR IGNORE`: an entity already flagged keeps its
    /// first `since`. All or none: the tap refuses the wallet write when
    /// this fails.
    pub fn flag_unverified(
        &self,
        wallet_id: &str,
        unverified: &[UnverifiedEntity],
        now: u64,
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            let mut flag = tx.prepare_cached(
                "INSERT OR IGNORE INTO dp_trust_unverified (wallet_id, kind, key, since)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for u in unverified {
                flag.execute(params![wallet_id, u.kind, u.key, now as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Every journal row of a wallet, oldest first.
    pub fn journal(&self, wallet_id: &str) -> Result<Vec<JournalRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, identity, kind, contact, ref, at, read_at FROM dp_events
             WHERE wallet_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([wallet_id], |r| {
            Ok(JournalRow {
                id: r.get(0)?,
                identity: r.get(1)?,
                kind: r.get(2)?,
                contact: r.get(3)?,
                reference: r.get(4)?,
                at: r.get::<_, i64>(5)? as u64,
                read_at: r.get::<_, Option<i64>>(6)?.map(|t| t as u64),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The wallet's `dp_trust_unverified` rows: `(kind, key, since)`, sorted.
    pub fn unverified(&self, wallet_id: &str) -> Result<Vec<(String, String, u64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT kind, key, since FROM dp_trust_unverified WHERE wallet_id = ?1
             ORDER BY kind, key",
        )?;
        let rows = stmt.query_map([wallet_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The `dp_registration` row whose asset lock is `outpoint`, in the
    /// canonical text (lower-case hex txid, `:`, decimal vout).
    pub fn registration_for_lock(&self, wallet_id: &str, outpoint: &str) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id FROM dp_registration
                 WHERE wallet_id = ?1 AND asset_lock_outpoint = ?2",
                params![wallet_id, outpoint],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Starts or updates the watch of one of the identity's contested labels.
    pub fn watch_contest(
        &self,
        wallet_id: &str,
        identity: &str,
        label: &str,
        ends_at: u64,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT INTO dp_contest_watch (wallet_id, identity, label, ends_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (wallet_id, identity, label) DO UPDATE SET ends_at = excluded.ends_at",
            params![wallet_id, identity, label, ends_at as i64],
        )?;
        Ok(())
    }

    /// The identity's watched contests: `(label, ends_at)`.
    pub fn contest_watches(&self, wallet_id: &str, identity: &str) -> Result<Vec<(String, u64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT label, ends_at FROM dp_contest_watch
             WHERE wallet_id = ?1 AND identity = ?2 ORDER BY label",
        )?;
        let rows = stmt.query_map([wallet_id, identity], |r| {
            Ok((r.get(0)?, r.get::<_, i64>(1)? as u64))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: &str = "aa";

    fn entry(kind: &'static str, contact: &str, reference: &str) -> JournalEntry {
        JournalEntry {
            identity: "me".into(),
            kind,
            contact: contact.into(),
            reference: reference.into(),
            read_at: None,
            unless_any: &[],
        }
    }

    #[test]
    fn a_changeset_written_twice_journals_once_and_keeps_its_read_state() {
        let db = AppDb::open_in_memory().unwrap();
        let events = [
            entry("payment_received", "bob", "t1"),
            entry("request_received", "bob", "5"),
        ];
        assert_eq!(db.record_events(W, &events, 10).unwrap(), 2);
        assert_eq!(db.record_events(W, &events, 20).unwrap(), 0);
        // A replay as catch-up does not mark an unread row read.
        let read = [JournalEntry {
            read_at: Some(30),
            ..entry("payment_received", "bob", "t1")
        }];
        assert_eq!(db.record_events(W, &read, 30).unwrap(), 0);
        let rows = db.journal(W).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.at == 10 && r.read_at.is_none()));
        assert_eq!(rows[0].reference, "t1");
    }

    #[test]
    fn an_exclusive_event_is_skipped_once_a_listed_kind_exists_for_the_contact() {
        let db = AppDb::open_in_memory().unwrap();
        const EXCLUSIVE: &[&str] = &["request_accepted", "contact_established"];
        let established = |kind, contact: &str, reference: &str| JournalEntry {
            unless_any: EXCLUSIVE,
            ..entry(kind, contact, reference)
        };
        db.record_events(W, &[established("request_accepted", "bob", "1")], 1)
            .unwrap();
        // A rotated request: another ref, or the other kind, is no news.
        let rotated = [
            established("request_accepted", "bob", "2"),
            established("contact_established", "bob", "3"),
            // Another contact, or an event without the rule, still lands.
            established("contact_established", "carol", "3"),
            entry("payment_received", "bob", "t1"),
        ];
        assert_eq!(db.record_events(W, &rotated, 2).unwrap(), 2);
        let kinds: Vec<(String, String)> = db
            .journal(W)
            .unwrap()
            .into_iter()
            .map(|r| (r.kind, r.contact))
            .collect();
        assert_eq!(
            kinds,
            [
                ("request_accepted", "bob"),
                ("contact_established", "carol"),
                ("payment_received", "bob"),
            ]
            .map(|(k, c)| (k.to_string(), c.to_string()))
        );
    }

    #[test]
    fn unverified_rows_keep_their_first_since() {
        let db = AppDb::open_in_memory().unwrap();
        let flags = [UnverifiedEntity {
            kind: "identity",
            key: "x".into(),
        }];
        db.flag_unverified(W, &flags, 5).unwrap();
        db.flag_unverified(W, &flags, 9).unwrap();
        assert_eq!(
            db.unverified(W).unwrap(),
            vec![("identity".to_string(), "x".to_string(), 5)]
        );
    }

    #[test]
    fn busy_and_full_databases_are_transient() {
        use rusqlite::ffi::{self, Error as FfiError};
        let failure = |code| {
            crate::AppDbError::Sqlite(rusqlite::Error::SqliteFailure(FfiError::new(code), None))
        };
        assert!(failure(ffi::SQLITE_BUSY).is_transient());
        assert!(failure(ffi::SQLITE_FULL).is_transient());
        assert!(!failure(ffi::SQLITE_CONSTRAINT).is_transient());
        assert!(!crate::AppDbError::Corrupt("x".into()).is_transient());
    }

    #[test]
    fn registration_and_contest_lookups() {
        let db = AppDb::open_in_memory().unwrap();
        db.conn()
            .execute(
                "INSERT INTO dp_registration
                     (wallet_id, label, funding, asset_lock_outpoint, phase, created_at, updated_at)
                 VALUES (?1, 'alice', '{}', 'ab:1', 'funding_sent', 1, 1)",
                [W],
            )
            .unwrap();
        let id = db.registration_for_lock(W, "ab:1").unwrap();
        assert!(id.is_some());
        assert_eq!(db.registration_for_lock(W, "ab:0").unwrap(), None);
        assert_eq!(db.registration_for_lock("bb", "ab:1").unwrap(), None);

        db.watch_contest(W, "me", "alice", 100).unwrap();
        db.watch_contest(W, "me", "alice", 200).unwrap();
        assert_eq!(
            db.contest_watches(W, "me").unwrap(),
            vec![("alice".to_string(), 200)]
        );
        assert!(db.contest_watches(W, "other").unwrap().is_empty());
    }
}
