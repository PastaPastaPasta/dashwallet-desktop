//! What the changeset tap (DASHPAY §3.5, ROADMAP E0-06) writes to the
//! `dp_events` journal, and the rows it reads to classify a changeset.

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

/// The newest-first page of one identity's journal rows below a cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPage {
    pub rows: Vec<JournalRow>,
    /// More rows are older than the last one.
    pub more: bool,
}

impl AppDb {
    /// Writes one changeset's journal events in one transaction, `INSERT OR
    /// IGNORE`: an event already journaled keeps its row (and its read
    /// state), and a removed wallet's insert is skipped by its trigger.
    /// Returns the events this write added, as `(index in events, row id)`:
    /// only these are news (DP2-05).
    pub fn record_events(
        &self,
        wallet_id: &str,
        events: &[JournalEntry],
        now: u64,
    ) -> Result<Vec<(usize, i64)>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut added = Vec::new();
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
            for (i, e) in events.iter().enumerate() {
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
                let inserted = insert.execute(params![
                    wallet_id,
                    e.identity,
                    e.kind,
                    e.contact,
                    e.reference,
                    now as i64,
                    e.read_at.map(|t| t as i64),
                ])?;
                if inserted == 1 {
                    added.push((i, tx.last_insert_rowid()));
                }
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// The identity's rows with an id below `before` (all with `None`),
    /// newest first, at most `limit`, leaving out rows whose contact is in
    /// `skip_contacts` (ignored senders).
    pub fn journal_page(
        &self,
        wallet_id: &str,
        identity: &str,
        before: Option<i64>,
        limit: usize,
        skip_contacts: &[String],
    ) -> Result<JournalPage> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT id, identity, kind, contact, ref, at, read_at FROM dp_events
             WHERE wallet_id = ?1 AND identity = ?2 AND id < ?3 ORDER BY id DESC",
        )?;
        let before = before.unwrap_or(i64::MAX);
        let mut rows = stmt.query(params![wallet_id, identity, before])?;
        let (mut out, mut more) = (Vec::new(), false);
        while let Some(r) = rows.next()? {
            let row = JournalRow {
                id: r.get(0)?,
                identity: r.get(1)?,
                kind: r.get(2)?,
                contact: r.get(3)?,
                reference: r.get(4)?,
                at: r.get::<_, i64>(5)? as u64,
                read_at: r.get::<_, Option<i64>>(6)?.map(|t| t as u64),
            };
            if skip_contacts.contains(&row.contact) {
                continue;
            }
            if out.len() == limit {
                more = true;
                break;
            }
            out.push(row);
        }
        Ok(JournalPage { rows: out, more })
    }

    /// Marks the identity's unread rows with id ≤ `up_to` read at `now`;
    /// with `contact`, only that contact's (an ignored sender's). How many
    /// rows changed.
    pub fn mark_events_read(
        &self,
        wallet_id: &str,
        identity: &str,
        up_to: i64,
        contact: Option<&str>,
        now: u64,
    ) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE dp_events SET read_at = ?4
             WHERE wallet_id = ?1 AND identity = ?2 AND id <= ?3 AND read_at IS NULL
               AND (?5 IS NULL OR contact = ?5)",
            params![wallet_id, identity, up_to, now as i64, contact],
        )?)
    }

    /// The wallet's unread rows per identity (the bell counts).
    pub fn unread_counts(&self, wallet_id: &str) -> Result<Vec<(String, u32)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT identity, count(*) FROM dp_events
             WHERE wallet_id = ?1 AND read_at IS NULL GROUP BY identity ORDER BY identity",
        )?;
        let rows = stmt.query_map([wallet_id], |r| {
            let n: i64 = r.get(1)?;
            Ok((r.get(0)?, u32::try_from(n).unwrap_or(u32::MAX)))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
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

    /// Each of the wallet's registration drafts with its identity index:
    /// `(id, identity_index)`, by id (E0-04 §2a.5: the draft's own funding
    /// key, kept here so a deleted or rolled-back dispatch journal still
    /// finds its tracked asset-lock row). Drafts without an index yet are
    /// left out: they never funded.
    pub fn registration_identity_indices(&self, wallet_id: &str) -> Result<Vec<(i64, u32)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, identity_index FROM dp_registration
             WHERE wallet_id = ?1 AND identity_index IS NOT NULL ORDER BY id",
        )?;
        let rows = stmt.query_map(params![wallet_id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, index) = row?;
            // The CHECK keeps it non-negative; an index past u32 is no
            // DIP-13 index, so no tracked row can carry it.
            if let Ok(index) = u32::try_from(index) {
                out.push((id, index));
            }
        }
        Ok(out)
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
        assert_eq!(db.record_events(W, &events, 10).unwrap().len(), 2);
        assert!(db.record_events(W, &events, 20).unwrap().is_empty());
        // A replay as catch-up does not mark an unread row read.
        let read = [JournalEntry {
            read_at: Some(30),
            ..entry("payment_received", "bob", "t1")
        }];
        assert!(db.record_events(W, &read, 30).unwrap().is_empty());
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
        let added = db.record_events(W, &rotated, 2).unwrap();
        assert_eq!(added.iter().map(|a| a.0).collect::<Vec<_>>(), [2, 3]);
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
    fn a_write_reports_the_rows_it_added_and_a_removed_wallet_adds_none() {
        let db = AppDb::open_in_memory().unwrap();
        let first = db
            .record_events(W, &[entry("request_received", "bob", "1")], 1)
            .unwrap();
        let events = [
            entry("request_received", "bob", "1"),
            entry("request_received", "carol", "2"),
        ];
        let added = db.record_events(W, &events, 2).unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].0, 1);
        assert!(added[0].1 > first[0].1);
        let ids: Vec<i64> = db.journal(W).unwrap().iter().map(|r| r.id).collect();
        assert_eq!(ids, [first[0].1, added[0].1]);

        // A write still under way when the wallet goes leaves nothing.
        db.delete_wallet(W).unwrap();
        let late = [entry("request_received", "dave", "3")];
        assert!(db.record_events(W, &late, 3).unwrap().is_empty());
        assert!(db.journal(W).unwrap().is_empty());
        assert!(db.unread_counts(W).unwrap().is_empty());
    }

    #[test]
    fn pages_read_state_and_counts() {
        let db = AppDb::open_in_memory().unwrap();
        let mut events: Vec<JournalEntry> = (0..5)
            .map(|n| entry("request_received", &format!("c{n}"), &n.to_string()))
            .collect();
        events[0].read_at = Some(1);
        events.push(JournalEntry {
            identity: "other".into(),
            ..entry("request_received", "c9", "9")
        });
        let ids: Vec<i64> = (db.record_events(W, &events, 5).unwrap().iter())
            .map(|a| a.1)
            .collect();
        assert_eq!(
            db.unread_counts(W).unwrap(),
            [("me".to_string(), 4), ("other".to_string(), 1)]
        );

        // Newest first, `limit` at a time, below the cursor; skipped
        // contacts neither show nor use up the limit.
        let skip = ["c3".to_string()];
        let page = db.journal_page(W, "me", None, 2, &skip).unwrap();
        let shown = |p: &JournalPage| p.rows.iter().map(|r| r.contact.clone()).collect::<Vec<_>>();
        assert_eq!(shown(&page), ["c4", "c2"]);
        assert!(page.more);
        let next = db
            .journal_page(W, "me", Some(page.rows[1].id), 2, &skip)
            .unwrap();
        assert_eq!(shown(&next), ["c1", "c0"]);
        assert!(!next.more);
        assert!(next.rows[1].read_at.is_some());
        let all = db.journal_page(W, "me", None, 10, &[]).unwrap();
        assert_eq!(all.rows.len(), 5);
        assert!(
            db.journal_page("bb", "me", None, 10, &[])
                .unwrap()
                .rows
                .is_empty()
        );

        // One contact's rows, then everything up to an id.
        assert_eq!(
            db.mark_events_read(W, "me", i64::MAX, Some("c3"), 7)
                .unwrap(),
            1
        );
        assert_eq!(db.mark_events_read(W, "me", ids[2], None, 8).unwrap(), 2);
        assert_eq!(db.mark_events_read(W, "me", ids[2], None, 9).unwrap(), 0);
        let read: Vec<Option<u64>> = (db.journal(W).unwrap().iter())
            .filter(|r| r.identity == "me")
            .map(|r| r.read_at)
            .collect();
        assert_eq!(read, [Some(1), Some(8), Some(8), Some(7), None]);
        assert_eq!(
            db.unread_counts(W).unwrap(),
            [("me".to_string(), 1), ("other".to_string(), 1)]
        );
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

        assert!(db.registration_identity_indices(W).unwrap().is_empty());
        for (index, label) in [("7", "bob"), ("NULL", "carol"), ("2", "dave")] {
            db.conn()
                .execute(
                    &format!(
                        "INSERT INTO dp_registration
                             (wallet_id, identity_index, label, funding, phase, created_at, updated_at)
                         VALUES (?1, {index}, '{label}', '{{}}', 'draft', 1, 1)"
                    ),
                    [W],
                )
                .unwrap();
        }
        let first = id.unwrap();
        assert_eq!(
            db.registration_identity_indices(W).unwrap(),
            vec![(first + 1, 7), (first + 3, 2)],
            "drafts with an index, by id"
        );
        assert!(db.registration_identity_indices("bb").unwrap().is_empty());

        db.watch_contest(W, "me", "alice", 100).unwrap();
        db.watch_contest(W, "me", "alice", 200).unwrap();
        assert_eq!(
            db.contest_watches(W, "me").unwrap(),
            vec![("alice".to_string(), 200)]
        );
        assert!(db.contest_watches(W, "other").unwrap().is_empty());
    }
}
