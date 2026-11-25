//! Version 12: what a fork-written `shielded.sqlite` drains into before the
//! upstream store first opens it.
//!
//! The fork's `shielded_spend_dispositions` kept each executed or unresolved
//! spend's notes out of selection until the engine acknowledged it, and its
//! `unsent` column proved a record was never broadcast. Upstream reads
//! neither, so the drain moves that evidence here: every held list becomes
//! `shielded_drained_hold` rows, lifted only when the journal reads the note
//! spent; every disposition this build cannot decode is named in
//! `shielded_drained_unreadable`; artifacts are settled by guarded updates
//! that never lower a status; and `shielded_drain` marks the drain's
//! progress, so a crash between its steps resumes.

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::DispatchJournal;
use crate::{AppDbError, Result};

/// The tables of version 12. `IF NOT EXISTS`, as the other shielded tables.
pub(super) const SHIELDED_DRAIN: &str = "
CREATE TABLE IF NOT EXISTS shielded_drained_hold (
  wallet    BLOB NOT NULL CHECK (length(wallet) = 32),
  account   INTEGER NOT NULL CHECK (account BETWEEN 0 AND 4294967295),
  st_hash   BLOB NOT NULL CHECK (length(st_hash) = 32),
  nullifier BLOB NOT NULL CHECK (length(nullifier) = 32),
  origin    TEXT NOT NULL CHECK (origin IN ('executed', 'unresolved', 'unreadable')),
  kind      TEXT NOT NULL,
  reason    TEXT,
  spent     INTEGER NOT NULL DEFAULT 0 CHECK (spent IN (0, 1)),
  PRIMARY KEY (wallet, st_hash, nullifier)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS shielded_drained_unreadable (
  wallet  BLOB NOT NULL CHECK (length(wallet) = 32),
  st_hash BLOB NOT NULL CHECK (length(st_hash) = 32),
  kind    TEXT NOT NULL,
  reason  TEXT,
  PRIMARY KEY (wallet, st_hash)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS shielded_drain (
  store TEXT PRIMARY KEY,
  progress TEXT NOT NULL CHECK (progress IN ('journaled', 'done'))
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS shielded_drained_release (
  wallet        BLOB NOT NULL CHECK (length(wallet) = 32),
  st_hash       BLOB NOT NULL CHECK (length(st_hash) = 32),
  recorded_at   INTEGER CHECK (recorded_at IS NULL OR recorded_at >= 0),
  released_pass INTEGER,
  released_at   INTEGER,
  CHECK ((released_pass IS NULL) = (released_at IS NULL)),
  PRIMARY KEY (wallet, st_hash)
) WITHOUT ROWID;
";

/// Version 11 → 12: the tables only.
pub(super) const MIGRATE_V11: &str = "UPDATE meta SET v = 12 WHERE k = 'schema';";

/// How far a store's drain got ([`DispatchJournal::shielded_drain_state`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShieldedDrainState {
    /// The journal holds everything the store's fork rows said; the store
    /// may not have retired them yet.
    Journaled,
    /// The store retired its fork rows after the journal took them.
    Done,
}

/// What one fork disposition says, decoded by the fork's own rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrainedOutcome {
    Executed,
    /// `refused`, `abandoned`, `rejected` or `pruned`.
    NotExecuted(String),
    /// `deserialize_dropped`, `purged`, `unattributable` or `cancelled`.
    Unresolved(String),
    /// A kind or reason this build does not decode.
    Unreadable,
}

/// One fork disposition: its key, its stored strings verbatim, the notes it
/// held, and what it decodes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainedDisposition {
    pub wallet: [u8; 32],
    pub account: u32,
    pub st_hash: [u8; 32],
    pub kind: String,
    pub reason: Option<String>,
    pub held: Vec<[u8; 32]>,
    pub outcome: DrainedOutcome,
}

/// A fork record durably never broadcast (`unsent = 1`): its transition was
/// never sent, so it is settled `not_executed`/`abandoned`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeverSentSpend {
    pub wallet: [u8; 32],
    pub activity: [u8; 32],
    /// The hash of its stored transition.
    pub st_hash: [u8; 32],
}

/// A drained unresolved or unreadable spend a completed pass may prove can
/// never execute (Q110 110-3), as the journal reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainedReleaseCandidate {
    pub wallet: [u8; 32],
    pub account: u32,
    pub st_hash: [u8; 32],
    /// The anchor of the journal's artifacts of its transition; `None` when
    /// none names one, or they name more than one.
    pub anchor: Option<[u8; 32]>,
    /// The height a completed pass recorded its anchor at
    /// ([`DispatchJournal::record_drained_anchor_height`]).
    pub recorded_at: Option<u64>,
    /// Every note it held.
    pub nullifiers: Vec<[u8; 32]>,
    /// The journal reads one of its notes spent, its transition executed,
    /// or its holds under more than one account.
    pub flagged: bool,
}

/// Everything one drain writes to the journal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShieldedDrainPlan {
    pub dispositions: Vec<DrainedDisposition>,
    pub never_sent: Vec<NeverSentSpend>,
}

impl DispatchJournal {
    /// How far the drain of the store named `store` got, or `None` when it
    /// never wrote here.
    pub fn shielded_drain_state(&self, store: &str) -> Result<Option<ShieldedDrainState>> {
        let state: Option<String> = self
            .conn()
            .query_row(
                "SELECT progress FROM shielded_drain WHERE store = ?1",
                params![store],
                |r| r.get(0),
            )
            .optional()?;
        match state.as_deref() {
            None => Ok(None),
            Some("journaled") => Ok(Some(ShieldedDrainState::Journaled)),
            Some("done") => Ok(Some(ShieldedDrainState::Done)),
            Some(other) => Err(AppDbError::Corrupt(format!(
                "shielded_drain progress {other}"
            ))),
        }
    }

    /// Writes `plan` and marks `store`'s drain `journaled`, in one
    /// transaction. Every write repeats as a no-op, so a drain that crashed
    /// after this runs it again unchanged.
    ///
    /// - Each executed, unresolved or unreadable disposition's held list
    ///   goes in as `shielded_drained_hold` rows, whether or not an artifact
    ///   names its transition. A row is written spent when an artifact note
    ///   or an earlier drained hold of the same wallet already reads that
    ///   nullifier spent; a row already there keeps its flag.
    /// - An unreadable one is named in `shielded_drained_unreadable`.
    /// - The artifacts of its transition are settled by guarded updates:
    ///   `executed` over `unclassified`, `registered` or `unresolved`;
    ///   `unresolved` and `not_executed` over `unclassified` or `registered`
    ///   only; an unreadable one moves `unclassified` or `registered` to
    ///   `unresolved` and records no disposition. A status is never lowered,
    ///   and the drained holds stand whatever the artifact says.
    /// - A never-sent record's artifact, matched on its activity and
    ///   transition, is settled `not_executed`/`abandoned` from
    ///   `unclassified` or `registered`.
    pub fn record_shielded_drain(&self, store: &str, plan: &ShieldedDrainPlan) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for d in &plan.dispositions {
            let origin = match &d.outcome {
                DrainedOutcome::Executed => Some("executed"),
                DrainedOutcome::Unresolved(_) => Some("unresolved"),
                DrainedOutcome::Unreadable => Some("unreadable"),
                DrainedOutcome::NotExecuted(_) => None,
            };
            if let Some(origin) = origin {
                for nullifier in &d.held {
                    tx.execute(
                        "INSERT INTO shielded_drained_hold \
                         (wallet, account, st_hash, nullifier, origin, kind, reason, spent) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, EXISTS (SELECT 1 FROM \
                         shielded_artifact_note WHERE wallet = ?1 AND nullifier = ?4 \
                         AND spent = 1) OR EXISTS (SELECT 1 FROM shielded_drained_hold \
                         WHERE wallet = ?1 AND nullifier = ?4 AND spent = 1)) \
                         ON CONFLICT (wallet, st_hash, nullifier) DO NOTHING",
                        params![
                            &d.wallet[..],
                            d.account,
                            &d.st_hash[..],
                            &nullifier[..],
                            origin,
                            d.kind,
                            d.reason
                        ],
                    )?;
                }
            }
            if d.outcome == DrainedOutcome::Unreadable {
                tx.execute(
                    "INSERT INTO shielded_drained_unreadable (wallet, st_hash, kind, reason) \
                     VALUES (?1, ?2, ?3, ?4) ON CONFLICT (wallet, st_hash) DO NOTHING",
                    params![&d.wallet[..], &d.st_hash[..], d.kind, d.reason],
                )?;
            }
            let (status, from, kind, reason): (&str, &str, Option<&str>, Option<&str>) =
                match &d.outcome {
                    DrainedOutcome::Executed => (
                        "executed",
                        "('unclassified', 'registered', 'unresolved')",
                        Some("executed"),
                        None,
                    ),
                    DrainedOutcome::NotExecuted(reason) => (
                        "not_executed",
                        "('unclassified', 'registered')",
                        Some("not_executed"),
                        Some(reason),
                    ),
                    DrainedOutcome::Unresolved(reason) => (
                        "unresolved",
                        "('unclassified', 'registered')",
                        Some("unresolved"),
                        Some(reason),
                    ),
                    DrainedOutcome::Unreadable => {
                        ("unresolved", "('unclassified', 'registered')", None, None)
                    }
                };
            tx.execute(
                &format!(
                    "UPDATE shielded_artifact SET status = ?3, status_reason = NULL, \
                     disposition_kind = ?4, disposition_reason = ?5 \
                     WHERE wallet = ?1 AND st_hash = ?2 AND status IN {from}"
                ),
                params![&d.wallet[..], &d.st_hash[..], status, kind, reason],
            )?;
        }
        for spend in &plan.never_sent {
            tx.execute(
                "UPDATE shielded_artifact SET status = 'not_executed', status_reason = NULL, \
                 disposition_kind = 'not_executed', disposition_reason = 'abandoned' \
                 WHERE wallet = ?1 AND activity = ?2 AND st_hash = ?3 \
                 AND status IN ('unclassified', 'registered')",
                params![&spend.wallet[..], &spend.activity[..], &spend.st_hash[..]],
            )?;
        }
        tx.execute(
            "INSERT INTO shielded_drain (store, progress) VALUES (?1, 'journaled') \
             ON CONFLICT (store) DO UPDATE SET progress = 'journaled'",
            params![store],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Marks `store`'s drain done once the store retired its fork rows.
    pub fn finish_shielded_drain(&self, store: &str) -> Result<()> {
        let updated = self.conn().execute(
            "UPDATE shielded_drain SET progress = 'done' WHERE store = ?1",
            params![store],
        )?;
        if updated == 0 {
            return Err(AppDbError::Corrupt(format!(
                "no journaled drain of {store} to finish"
            )));
        }
        Ok(())
    }

    /// The nullifiers `wallet`'s drained holds keep out of selection: those
    /// not yet read spent, of a spend not released, by account.
    pub fn shielded_drained_holds(&self, wallet: &[u8; 32]) -> Result<Vec<(u32, [u8; 32])>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT account, nullifier FROM shielded_drained_hold h \
             WHERE wallet = ?1 AND spent = 0 AND NOT EXISTS (SELECT 1 FROM \
             shielded_drained_release r WHERE r.wallet = h.wallet AND r.st_hash = h.st_hash \
             AND r.released_at IS NOT NULL) ORDER BY account, nullifier",
        )?;
        stmt.query_map(params![&wallet[..]], |r| {
            Ok((r.get::<_, u32>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?
        .map(|row| {
            let (account, nullifier) = row?;
            Ok((
                account,
                super::fixed(nullifier, "shielded_drained_hold.nullifier")?,
            ))
        })
        .collect()
    }

    /// The drained unresolved and unreadable spends not yet released, once
    /// `store`'s drain is done: only the drain writes them, before
    /// upstream's store first opens, so no spend of this build is one.
    pub fn shielded_drained_release_candidates(
        &self,
        store: &str,
    ) -> Result<Vec<DrainedReleaseCandidate>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT h.wallet, h.st_hash, h.account, h.nullifier, h.spent FROM \
             shielded_drained_hold h WHERE h.origin IN ('unresolved', 'unreadable') \
             AND EXISTS (SELECT 1 FROM shielded_drain WHERE store = ?1 AND progress = 'done') \
             AND NOT EXISTS (SELECT 1 FROM shielded_drained_release r WHERE \
             r.wallet = h.wallet AND r.st_hash = h.st_hash AND r.released_at IS NOT NULL) \
             ORDER BY h.wallet, h.st_hash, h.nullifier",
        )?;
        let rows = stmt
            .query_map(params![store], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, u32>(2)?,
                    r.get::<_, Vec<u8>>(3)?,
                    r.get::<_, bool>(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut out: Vec<DrainedReleaseCandidate> = Vec::new();
        for (wallet, st_hash, account, nullifier, spent) in rows {
            let wallet = super::fixed(wallet, "shielded_drained_hold.wallet")?;
            let st_hash = super::fixed(st_hash, "shielded_drained_hold.st_hash")?;
            let nullifier = super::fixed(nullifier, "shielded_drained_hold.nullifier")?;
            match out.last_mut() {
                Some(c) if c.wallet == wallet && c.st_hash == st_hash => {
                    c.flagged |= spent || c.account != account;
                    c.nullifiers.push(nullifier);
                }
                _ => out.push(DrainedReleaseCandidate {
                    wallet,
                    account,
                    st_hash,
                    anchor: None,
                    recorded_at: None,
                    nullifiers: vec![nullifier],
                    flagged: spent,
                }),
            }
        }
        for c in &mut out {
            let anchors: Vec<Vec<u8>> = conn
                .prepare(
                    "SELECT DISTINCT anchor FROM shielded_artifact WHERE wallet = ?1 \
                     AND st_hash = ?2 AND anchor IS NOT NULL",
                )?
                .query_map(params![&c.wallet[..], &c.st_hash[..]], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?;
            if let [anchor] = &anchors[..] {
                c.anchor = Some(super::fixed(anchor.clone(), "shielded_artifact.anchor")?);
            }
            c.flagged |= flags_spent(&conn, &c.wallet, &c.st_hash)?;
            c.recorded_at = conn
                .query_row(
                    "SELECT recorded_at FROM shielded_drained_release WHERE wallet = ?1 \
                     AND st_hash = ?2",
                    params![&c.wallet[..], &c.st_hash[..]],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .optional()?
                .flatten()
                .map(|h| {
                    u64::try_from(h).map_err(|_| {
                        AppDbError::Corrupt(format!("shielded_drained_release.recorded_at {h}"))
                    })
                })
                .transpose()?;
        }
        Ok(out)
    }

    /// Keeps the height a completed pass recorded `wallet`'s drained spend
    /// `st_hash`'s anchor at (Q110 110-3, E1). The first height kept stands.
    pub fn record_drained_anchor_height(
        &self,
        wallet: &[u8; 32],
        st_hash: &[u8; 32],
        height: u64,
    ) -> Result<()> {
        let height = i64::try_from(height)
            .map_err(|_| AppDbError::Corrupt(format!("an anchor height {height}")))?;
        self.conn().execute(
            "INSERT INTO shielded_drained_release (wallet, st_hash, recorded_at) \
             VALUES (?1, ?2, ?3) ON CONFLICT (wallet, st_hash) DO UPDATE SET \
             recorded_at = excluded.recorded_at WHERE recorded_at IS NULL",
            params![&wallet[..], &st_hash[..], height],
        )?;
        Ok(())
    }

    /// Releases `wallet`'s drained spend `st_hash`, which `pass` proved can
    /// never execute (Q110 110-3, E3): its holds no longer keep its notes,
    /// and its artifacts settle `not_executed`/`pruned` from any open
    /// status. In one transaction, and only while the journal still flags
    /// none of it; `false` when it flags one (nothing is written).
    pub fn release_drained_spend(
        &self,
        wallet: &[u8; 32],
        st_hash: &[u8; 32],
        pass: u64,
        now: u64,
    ) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if flags_spent(&tx, wallet, st_hash)? {
            return Ok(false);
        }
        let pass =
            i64::try_from(pass).map_err(|_| AppDbError::Corrupt(format!("a pass id {pass}")))?;
        let now = i64::try_from(now).map_err(|_| AppDbError::Corrupt(format!("a time {now}")))?;
        tx.execute(
            "INSERT INTO shielded_drained_release (wallet, st_hash, released_pass, released_at) \
             VALUES (?1, ?2, ?3, ?4) ON CONFLICT (wallet, st_hash) DO UPDATE SET \
             released_pass = excluded.released_pass, released_at = excluded.released_at \
             WHERE released_at IS NULL",
            params![&wallet[..], &st_hash[..], pass, now],
        )?;
        tx.execute(
            "UPDATE shielded_artifact SET status = 'not_executed', status_reason = NULL, \
             disposition_kind = 'not_executed', disposition_reason = 'pruned' \
             WHERE wallet = ?1 AND st_hash = ?2 \
             AND status IN ('unclassified', 'registered', 'unresolved')",
            params![&wallet[..], &st_hash[..]],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Whether a drained hold or an unreadable drained disposition names
    /// `wallet`'s transition `st_hash`: such a transition is never admitted
    /// for a re-send.
    pub fn shielded_drain_names(&self, wallet: &[u8; 32], st_hash: &[u8; 32]) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS (SELECT 1 FROM shielded_drained_hold WHERE wallet = ?1 AND st_hash = ?2) \
             OR EXISTS (SELECT 1 FROM shielded_drained_unreadable WHERE wallet = ?1 AND st_hash = ?2)",
            params![&wallet[..], &st_hash[..]],
            |r| r.get(0),
        )?)
    }
}

/// Whether the journal reads a note of `wallet`'s drained spend `st_hash`
/// spent (its drained holds, or any artifact's notes), or an artifact of
/// that transition executed.
fn flags_spent(conn: &rusqlite::Connection, wallet: &[u8; 32], st_hash: &[u8; 32]) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM shielded_drained_hold WHERE wallet = ?1 AND st_hash = ?2 \
         AND spent = 1) OR EXISTS (SELECT 1 FROM shielded_artifact WHERE wallet = ?1 \
         AND st_hash = ?2 AND status = 'executed') OR EXISTS (SELECT 1 FROM \
         shielded_artifact_note n JOIN shielded_drained_hold h ON n.wallet = h.wallet \
         AND n.nullifier = h.nullifier WHERE h.wallet = ?1 AND h.st_hash = ?2 AND n.spent = 1)",
        params![&wallet[..], &st_hash[..]],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::super::{DISPATCH_DB_FILE, JournalOpen};
    use super::*;

    const W: [u8; 32] = [1; 32];

    fn ready(path: &std::path::Path) -> DispatchJournal {
        match DispatchJournal::open(path, 1).unwrap() {
            JournalOpen::Ready(j) => j,
            JournalOpen::NewerSchema(v) => panic!("newer schema {v}"),
        }
    }

    fn artifact(j: &DispatchJournal, activity: u8, st_hash: u8, status: &str) {
        j.conn()
            .execute(
                "INSERT INTO shielded_artifact (wallet, activity, origin, st_hash, kind, anchor, \
                 status, at) VALUES (?1, ?2, 'dispatched', ?3, 'transfer', ?4, ?5, 1)",
                params![
                    &W[..],
                    &[activity; 32][..],
                    &[st_hash; 32][..],
                    &[9u8; 32][..],
                    status
                ],
            )
            .unwrap();
    }

    fn status(j: &DispatchJournal, activity: u8) -> (String, Option<String>, Option<String>) {
        j.conn()
            .query_row(
                "SELECT status, disposition_kind, disposition_reason FROM shielded_artifact \
                 WHERE wallet = ?1 AND activity = ?2",
                params![&W[..], &[activity; 32][..]],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    }

    fn disposition(st_hash: u8, outcome: DrainedOutcome, held: &[u8]) -> DrainedDisposition {
        let (kind, reason) = match &outcome {
            DrainedOutcome::Executed => ("executed".to_owned(), None),
            DrainedOutcome::NotExecuted(r) => ("not_executed".to_owned(), Some(r.clone())),
            DrainedOutcome::Unresolved(r) => ("unresolved".to_owned(), Some(r.clone())),
            DrainedOutcome::Unreadable => ("unresolved".to_owned(), Some("later".to_owned())),
        };
        DrainedDisposition {
            wallet: W,
            account: 0,
            st_hash: [st_hash; 32],
            kind,
            reason,
            held: held.iter().map(|n| [*n; 32]).collect(),
            outcome,
        }
    }

    /// A schema 11 journal gains the drain's tables at 12 and keeps its rows.
    #[test]
    fn should_migrate_a_v11_journal_to_12_keeping_its_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DISPATCH_DB_FILE);
        {
            let j = ready(&path);
            artifact(&j, 1, 2, "registered");
            j.conn()
                .execute_batch(
                    "DROP TABLE shielded_drained_hold; DROP TABLE shielded_drained_unreadable;
                     DROP TABLE shielded_drain; DROP TABLE shielded_drained_release;
                     UPDATE meta SET v = 11 WHERE k = 'schema';",
                )
                .unwrap();
        }
        {
            let JournalOpen::Ready(j) = DispatchJournal::open_as_schema(&path, 1, 11).unwrap()
            else {
                panic!("a schema 11 build reads its own journal");
            };
            let tables: i64 = j
                .conn()
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name LIKE 'shielded_drain%'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(tables, 0, "a schema 11 journal has no drain tables");
        }
        let j = ready(&path);
        let schema: i64 = j
            .conn()
            .query_row("SELECT v FROM meta WHERE k = 'schema'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(schema, 12);
        assert_eq!(status(&j, 1).0, "registered");
        assert_eq!(j.shielded_drain_state("s").unwrap(), None);
        j.record_shielded_drain("s", &ShieldedDrainPlan::default())
            .unwrap();
        assert_eq!(
            j.shielded_drain_state("s").unwrap(),
            Some(ShieldedDrainState::Journaled)
        );
        j.finish_shielded_drain("s").unwrap();
        assert_eq!(
            j.shielded_drain_state("s").unwrap(),
            Some(ShieldedDrainState::Done)
        );
    }

    /// A drained hold stands until the journal reads its note spent, and a
    /// note already read spent is drained spent.
    #[test]
    fn should_lift_a_drained_hold_only_when_the_journal_reads_the_note_spent() {
        let dir = tempfile::tempdir().unwrap();
        let j = ready(&dir.path().join(DISPATCH_DB_FILE));
        artifact(&j, 1, 2, "registered");
        j.conn()
            .execute(
                "INSERT INTO shielded_artifact_note (wallet, activity, nullifier, spent) \
                 VALUES (?1, ?2, ?3, 1)",
                params![&W[..], &[1u8; 32][..], &[30u8; 32][..]],
            )
            .unwrap();
        let plan = ShieldedDrainPlan {
            dispositions: vec![disposition(2, DrainedOutcome::Executed, &[10, 11, 30])],
            never_sent: Vec::new(),
        };
        j.record_shielded_drain("s", &plan).unwrap();
        assert_eq!(
            j.shielded_drained_holds(&W).unwrap(),
            vec![(0, [10; 32]), (0, [11; 32])],
            "the note the journal already read spent is not held"
        );
        j.note_spent_nullifiers(&W, &[[10; 32]]).unwrap();
        assert_eq!(j.shielded_drained_holds(&W).unwrap(), vec![(0, [11; 32])]);
        j.record_shielded_drain("s", &plan).unwrap();
        assert_eq!(
            j.shielded_drained_holds(&W).unwrap(),
            vec![(0, [11; 32])],
            "a repeated drain keeps each row's flag"
        );
        assert!(j.shielded_drain_names(&W, &[2; 32]).unwrap());
        assert!(!j.shielded_drain_names(&W, &[3; 32]).unwrap());
    }

    /// The guarded updates never lower a status, and an unreadable
    /// disposition records no disposition and is named.
    #[test]
    fn should_settle_artifacts_without_lowering_a_status() {
        let dir = tempfile::tempdir().unwrap();
        let j = ready(&dir.path().join(DISPATCH_DB_FILE));
        artifact(&j, 1, 1, "unresolved");
        artifact(&j, 2, 2, "executed");
        artifact(&j, 3, 3, "unresolved");
        artifact(&j, 4, 4, "registered");
        artifact(&j, 5, 5, "archived");
        artifact(&j, 6, 6, "unclassified");
        let plan = ShieldedDrainPlan {
            dispositions: vec![
                disposition(1, DrainedOutcome::Executed, &[1]),
                disposition(2, DrainedOutcome::Unresolved("cancelled".into()), &[2]),
                disposition(3, DrainedOutcome::NotExecuted("refused".into()), &[]),
                disposition(4, DrainedOutcome::Unreadable, &[]),
                disposition(5, DrainedOutcome::Executed, &[5]),
            ],
            never_sent: vec![NeverSentSpend {
                wallet: W,
                activity: [6; 32],
                st_hash: [6; 32],
            }],
        };
        j.record_shielded_drain("s", &plan).unwrap();
        assert_eq!(
            status(&j, 1),
            ("executed".into(), Some("executed".into()), None)
        );
        assert_eq!(status(&j, 2).0, "executed", "executed is never lowered");
        assert_eq!(
            status(&j, 3).0,
            "unresolved",
            "not_executed over unresolved is refused"
        );
        assert_eq!(status(&j, 4), ("unresolved".into(), None, None));
        assert_eq!(status(&j, 5).0, "archived");
        assert_eq!(
            status(&j, 6),
            (
                "not_executed".into(),
                Some("not_executed".into()),
                Some("abandoned".into())
            )
        );
        assert!(
            j.shielded_drain_names(&W, &[4; 32]).unwrap(),
            "unreadable is named"
        );
        assert!(!j.shielded_drain_names(&W, &[3; 32]).unwrap());
        assert_eq!(
            j.shielded_drained_holds(&W).unwrap(),
            vec![(0, [1; 32]), (0, [2; 32]), (0, [5; 32])],
            "every executed or unresolved held list holds, whatever its artifact says"
        );
    }

    fn drained(j: &DispatchJournal, dispositions: Vec<DrainedDisposition>) {
        let plan = ShieldedDrainPlan {
            dispositions,
            never_sent: Vec::new(),
        };
        j.record_shielded_drain("s", &plan).unwrap();
    }

    fn candidates(j: &DispatchJournal) -> Vec<DrainedReleaseCandidate> {
        j.shielded_drained_release_candidates("s").unwrap()
    }

    /// A journal made at 12 before the release table gains it at its next
    /// open, and keeps its holds.
    #[test]
    fn should_add_the_release_table_to_a_v12_journal_made_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DISPATCH_DB_FILE);
        {
            let j = ready(&path);
            drained(
                &j,
                vec![disposition(
                    2,
                    DrainedOutcome::Unresolved("purged".into()),
                    &[10],
                )],
            );
            j.conn()
                .execute_batch("DROP TABLE shielded_drained_release")
                .unwrap();
        }
        let j = ready(&path);
        assert_eq!(j.shielded_drained_holds(&W).unwrap(), vec![(0, [10; 32])]);
        j.record_drained_anchor_height(&W, &[2; 32], 7).unwrap();
    }

    /// Only a done drain's unresolved and unreadable spends are candidates,
    /// each with every note it held, its artifact's one anchor, and a flag
    /// when the journal reads it spent or executed.
    #[test]
    fn should_offer_only_a_done_drains_unresolved_and_unreadable_spends() {
        let dir = tempfile::tempdir().unwrap();
        let j = ready(&dir.path().join(DISPATCH_DB_FILE));
        artifact(&j, 1, 2, "registered");
        artifact(&j, 3, 4, "registered");
        artifact(&j, 5, 6, "registered");
        artifact(&j, 7, 6, "registered");
        j.conn()
            .execute(
                "UPDATE shielded_artifact SET anchor = ?1 WHERE activity = ?2",
                params![&[8u8; 32][..], &[7u8; 32][..]],
            )
            .unwrap();
        drained(
            &j,
            vec![
                disposition(2, DrainedOutcome::Unresolved("purged".into()), &[11, 10]),
                disposition(4, DrainedOutcome::Unreadable, &[12]),
                disposition(6, DrainedOutcome::Unresolved("cancelled".into()), &[13]),
                disposition(14, DrainedOutcome::Unresolved("purged".into()), &[14]),
                disposition(15, DrainedOutcome::Executed, &[15]),
            ],
        );
        assert_eq!(
            candidates(&j),
            vec![],
            "no candidate before the drain is done"
        );
        j.finish_shielded_drain("s").unwrap();
        let c = candidates(&j);
        let keys: Vec<_> = c
            .iter()
            .map(|c| (c.st_hash[0], c.anchor.map(|a| a[0])))
            .collect();
        assert_eq!(
            keys,
            vec![(2, Some(9)), (4, Some(9)), (6, None), (14, None)],
            "executed is not one; two anchors or none name no anchor"
        );
        assert_eq!(c[0].nullifiers, vec![[10; 32], [11; 32]]);
        assert!(c.iter().all(|c| !c.flagged && c.recorded_at.is_none()));

        j.note_spent_nullifiers(&W, &[[10; 32]]).unwrap();
        assert!(candidates(&j)[0].flagged, "a note read spent flags it");
        j.conn()
            .execute(
                "UPDATE shielded_artifact SET status = 'executed' WHERE activity = ?1",
                params![&[3u8; 32][..]],
            )
            .unwrap();
        assert!(candidates(&j)[1].flagged, "an executed artifact flags it");
    }

    /// Held under two accounts, a spend is flagged.
    #[test]
    fn should_flag_a_spend_held_under_two_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let j = ready(&dir.path().join(DISPATCH_DB_FILE));
        artifact(&j, 1, 2, "registered");
        let mut other = disposition(2, DrainedOutcome::Unresolved("purged".into()), &[11]);
        other.account = 1;
        drained(
            &j,
            vec![
                disposition(2, DrainedOutcome::Unresolved("purged".into()), &[10]),
                other,
            ],
        );
        j.finish_shielded_drain("s").unwrap();
        assert!(candidates(&j)[0].flagged);
    }

    /// The first recorded height stands; a release frees the spend's notes,
    /// settles its artifact `not_executed`/`pruned`, and is kept; a spend
    /// the journal flags is not released.
    #[test]
    fn should_release_a_drained_spend_durably_unless_the_journal_flags_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DISPATCH_DB_FILE);
        {
            let j = ready(&path);
            artifact(&j, 1, 2, "unresolved");
            artifact(&j, 3, 4, "unresolved");
            drained(
                &j,
                vec![
                    disposition(2, DrainedOutcome::Unresolved("purged".into()), &[10, 11]),
                    disposition(4, DrainedOutcome::Unresolved("purged".into()), &[12]),
                ],
            );
            j.finish_shielded_drain("s").unwrap();
            j.record_drained_anchor_height(&W, &[2; 32], 40).unwrap();
            j.record_drained_anchor_height(&W, &[2; 32], 41).unwrap();
            assert_eq!(
                candidates(&j)[0].recorded_at,
                Some(40),
                "the first height stands"
            );

            j.note_spent_nullifiers(&W, &[[12; 32]]).unwrap();
            assert!(!j.release_drained_spend(&W, &[4; 32], 5, 100).unwrap());
            assert!(j.release_drained_spend(&W, &[2; 32], 5, 100).unwrap());
        }
        let j = ready(&path);
        assert_eq!(
            j.shielded_drained_holds(&W).unwrap(),
            vec![],
            "10 and 11 are free, 12 is spent"
        );
        assert_eq!(
            candidates(&j)
                .iter()
                .map(|c| c.st_hash[0])
                .collect::<Vec<_>>(),
            vec![4],
            "a released spend is no candidate"
        );
        assert_eq!(
            status(&j, 1),
            (
                "not_executed".into(),
                Some("not_executed".into()),
                Some("pruned".into())
            )
        );
        assert_eq!(
            status(&j, 3).0,
            "unresolved",
            "a flagged spend's artifact stays"
        );
        assert!(
            j.shielded_drain_names(&W, &[2; 32]).unwrap(),
            "a released spend is still never re-sent"
        );
    }
}
