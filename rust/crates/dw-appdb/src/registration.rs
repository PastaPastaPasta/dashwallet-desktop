//! The single writer of `dp_registration` (DASHPAY §3.4 "Registration rows",
//! ROADMAP DP1-02): every statement that writes the table is here, and the
//! asset-lock outpoint reaches the column only as a [`LockOutpoint`], the one
//! canonical text the byte-wise unique index needs.

use rusqlite::{OptionalExtension, Row, params};

use crate::{AppDb, AppDbError, Result};

/// An asset-lock outpoint in the canonical text of `dp_registration_lock`:
/// the lower-case hex txid in display order, `:`, the decimal vout
/// (`OutPoint`'s `Display`). The index is byte-wise, so `'AB…:0'` and
/// `'ab…:0'` would count as two flows: [`LockOutpoint::parse`] normalizes
/// upper-case hex and refuses every other spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LockOutpoint(String);

impl LockOutpoint {
    /// `None` unless `text` is 64 hex digits (either case), one `:`, and a
    /// `u32` in plain decimal (no sign, no leading zero, no whitespace).
    pub fn parse(text: &str) -> Option<Self> {
        let (txid, vout) = text.split_once(':')?;
        if txid.len() != 64 || !txid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        if vout.is_empty()
            || !vout.bytes().all(|b| b.is_ascii_digit())
            || (vout.len() > 1 && vout.starts_with('0'))
        {
            return None;
        }
        let vout: u32 = vout.parse().ok()?;
        Some(Self(format!("{}:{vout}", txid.to_ascii_lowercase())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LockOutpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One `dp_registration` row as stored. `phase` and `funding` are the
/// engine's encodings; this crate does not interpret them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationRow {
    pub id: i64,
    pub identity_index: Option<u32>,
    pub identity: Option<String>,
    pub label: String,
    pub temp_label: Option<String>,
    pub funding: String,
    pub initial_profile: Option<String>,
    pub asset_lock_outpoint: Option<String>,
    pub phase: String,
    /// The failure code; set exactly when the flow failed.
    pub error: Option<String>,
    pub retryable: bool,
    /// Why the name step ended without the name, on a flow whose identity
    /// exists (it settles: the flow ends `Done`).
    pub name_error: Option<String>,
    /// Why the initial profile was not published, the same way.
    pub profile_error: Option<String>,
    /// The generation of the claim that holds the row
    /// ([`AppDb::claim_registration`]).
    pub owner: u64,
    pub created_at: u64,
    pub updated_at: u64,
}

/// A row to insert at `Draft` (or, adopting a lock, at a later phase).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRegistration {
    pub identity_index: Option<u32>,
    pub identity: Option<String>,
    pub label: String,
    pub temp_label: Option<String>,
    pub funding: String,
    pub initial_profile: Option<String>,
    pub phase: String,
}

/// What one transition changes. `None` leaves a column as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistrationChange {
    pub phase: Option<String>,
    pub identity_index: Option<u32>,
    pub identity: Option<String>,
    pub outpoint: Option<LockOutpoint>,
    /// `Some(Some((code, retryable)))` fails the flow; `Some(None)` clears
    /// a failure (a resume).
    pub failure: Option<Option<(String, bool)>>,
    /// The `ProfileCreated` step consumed it.
    pub clear_initial_profile: bool,
    /// The library dropped the lock before it reached the wire (a
    /// definitive rejection): the flow is unfunded again.
    pub clear_outpoint: bool,
    /// Records why the name step ended without the name.
    pub name_error: Option<String>,
    /// Records why the initial profile was not published.
    pub profile_error: Option<String>,
}

/// How [`AppDb::adopt_registration_lock`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adoption {
    /// A new row holds the lock.
    Inserted(i64),
    /// The row that already held the lock.
    Existing(i64),
    /// No row holds the lock, and a row of the wallet holds the new row's
    /// identity index: nothing is written.
    IndexHeld,
}

/// How a compare-and-set update ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationUpdate {
    Applied,
    /// The row is gone, or no longer in the expected phase and failure
    /// state: another writer moved it first.
    Stale,
    /// Another flow of the wallet already holds the outpoint (the unique
    /// index): one asset lock funds one flow.
    LockTaken,
}

const COLUMNS: &str = "id, identity_index, identity, label, temp_label, funding, initial_profile,
     asset_lock_outpoint, phase, error, retryable, owner, created_at, updated_at, name_error,
     profile_error";

fn read_row(r: &Row<'_>) -> rusqlite::Result<RegistrationRow> {
    Ok(RegistrationRow {
        id: r.get(0)?,
        identity_index: r.get::<_, Option<i64>>(1)?.map(|i| i as u32),
        identity: r.get(2)?,
        label: r.get(3)?,
        temp_label: r.get(4)?,
        funding: r.get(5)?,
        initial_profile: r.get(6)?,
        asset_lock_outpoint: r.get(7)?,
        phase: r.get(8)?,
        error: r.get(9)?,
        retryable: r.get::<_, i64>(10)? != 0,
        owner: r.get::<_, i64>(11)? as u64,
        created_at: r.get::<_, i64>(12)? as u64,
        updated_at: r.get::<_, i64>(13)? as u64,
        name_error: r.get(14)?,
        profile_error: r.get(15)?,
    })
}

fn is_lock_conflict(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, Some(msg))
            if f.code == rusqlite::ErrorCode::ConstraintViolation
                && msg.contains("dp_registration.asset_lock_outpoint")
    )
}

impl AppDb {
    /// Inserts a flow and returns its id.
    pub fn insert_registration(
        &self,
        wallet_id: &str,
        row: &NewRegistration,
        now: u64,
    ) -> Result<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO dp_registration
                 (wallet_id, identity_index, identity, label, temp_label, funding,
                  initial_profile, phase, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![
                wallet_id,
                row.identity_index.map(i64::from),
                row.identity,
                row.label,
                row.temp_label,
                row.funding,
                row.initial_profile,
                row.phase,
                now as i64,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// The flow that holds `outpoint`, or a new one at `row` holding it:
    /// keyed on the lock with the unique index's own predicate, so a
    /// stranded lock's row is reused, never doubled (§3.4 rule 5). A new
    /// row never takes an identity index a row of the wallet holds, in the
    /// same statement: two keys there would be one.
    pub fn adopt_registration_lock(
        &self,
        wallet_id: &str,
        outpoint: &LockOutpoint,
        row: &NewRegistration,
        now: u64,
    ) -> Result<Adoption> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let inserted: Option<i64> = tx
            .query_row(
                "INSERT INTO dp_registration
                     (wallet_id, identity_index, identity, label, temp_label, funding,
                      initial_profile, asset_lock_outpoint, phase, created_at, updated_at)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10
                 WHERE ?2 IS NULL OR NOT EXISTS (
                     SELECT 1 FROM dp_registration WHERE wallet_id = ?1 AND identity_index = ?2)
                 ON CONFLICT (wallet_id, asset_lock_outpoint)
                     WHERE asset_lock_outpoint IS NOT NULL
                 DO NOTHING
                 RETURNING id",
                params![
                    wallet_id,
                    row.identity_index.map(i64::from),
                    row.identity,
                    row.label,
                    row.temp_label,
                    row.funding,
                    row.initial_profile,
                    outpoint.as_str(),
                    row.phase,
                    now as i64,
                ],
                |r| r.get(0),
            )
            .optional()?;
        let adopted = match inserted {
            Some(id) => Adoption::Inserted(id),
            None => tx
                .query_row(
                    "SELECT id FROM dp_registration
                     WHERE wallet_id = ?1 AND asset_lock_outpoint = ?2",
                    params![wallet_id, outpoint.as_str()],
                    |r| r.get(0),
                )
                .optional()?
                .map_or(Adoption::IndexHeld, Adoption::Existing),
        };
        tx.commit()?;
        Ok(adopted)
    }

    /// Every flow of the wallet, oldest first.
    pub fn registration_rows(&self, wallet_id: &str) -> Result<Vec<RegistrationRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM dp_registration WHERE wallet_id = ?1 ORDER BY id"
        ))?;
        let rows = stmt.query_map([wallet_id], read_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn registration_row(&self, wallet_id: &str, id: i64) -> Result<Option<RegistrationRow>> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT {COLUMNS} FROM dp_registration WHERE wallet_id = ?1 AND id = ?2"),
                params![wallet_id, id],
                read_row,
            )
            .optional()?)
    }

    /// Takes the row for a new claim: raises its `owner` and returns the new
    /// generation, which every later write of the claim names. `None` if
    /// the row is gone.
    pub fn claim_registration(&self, wallet_id: &str, id: i64) -> Result<Option<u64>> {
        Ok(self
            .conn()
            .query_row(
                "UPDATE dp_registration SET owner = owner + 1
                 WHERE wallet_id = ?1 AND id = ?2
                 RETURNING owner",
                params![wallet_id, id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .map(|owner| owner as u64))
    }

    /// Applies `change` if the row is still in `phase` with the failure
    /// state `failed` (compare and set), and still held by `owner`: one
    /// transition, one writer. Evidence (a lock's outpoint the library
    /// tracked) is written under the owner read with it (DEC-173).
    #[expect(
        clippy::too_many_arguments,
        reason = "the compare-and-set's expected owner, phase and failure"
    )]
    pub fn update_registration(
        &self,
        wallet_id: &str,
        id: i64,
        owner: u64,
        phase: &str,
        failed: bool,
        change: &RegistrationChange,
        now: u64,
    ) -> Result<RegistrationUpdate> {
        let (set_failure, error, retryable) = match &change.failure {
            None => (false, None, false),
            Some(None) => (true, None, false),
            Some(Some((code, retryable))) => (true, Some(code.as_str()), *retryable),
        };
        let written = self.conn().execute(
            "UPDATE dp_registration SET
                 phase = COALESCE(?4, phase),
                 identity_index = COALESCE(?5, identity_index),
                 identity = COALESCE(?6, identity),
                 asset_lock_outpoint = CASE WHEN ?14 THEN NULL ELSE COALESCE(?7, asset_lock_outpoint) END,
                 error = CASE WHEN ?8 THEN ?9 ELSE error END,
                 retryable = CASE WHEN ?8 THEN ?10 ELSE retryable END,
                 initial_profile = CASE WHEN ?11 THEN NULL ELSE initial_profile END,
                 name_error = COALESCE(?16, name_error),
                 profile_error = COALESCE(?17, profile_error),
                 updated_at = ?12
             WHERE wallet_id = ?1 AND id = ?2 AND phase = ?3 AND (error IS NOT NULL) = ?13
                 AND owner = ?15",
            params![
                wallet_id,
                id,
                phase,
                change.phase,
                change.identity_index.map(i64::from),
                change.identity,
                change.outpoint.as_ref().map(LockOutpoint::as_str),
                set_failure,
                error,
                retryable,
                change.clear_initial_profile,
                now as i64,
                failed,
                change.clear_outpoint,
                owner as i64,
                change.name_error,
                change.profile_error,
            ],
        );
        match written {
            Ok(1) => Ok(RegistrationUpdate::Applied),
            Ok(_) => Ok(RegistrationUpdate::Stale),
            Err(e) if is_lock_conflict(&e) => Ok(RegistrationUpdate::LockTaken),
            Err(e) => Err(AppDbError::from(e)),
        }
    }

    /// Deletes a flow (a discard) held by `owner`, if it is still in
    /// `phase` with the lock `outpoint` it was judged on: a lock recorded
    /// since (the tap, "Finish transfers") keeps it. `false` if it was not
    /// deleted.
    pub fn delete_registration(
        &self,
        wallet_id: &str,
        id: i64,
        owner: u64,
        phase: &str,
        outpoint: Option<&str>,
    ) -> Result<bool> {
        Ok(self.conn().execute(
            "DELETE FROM dp_registration
             WHERE wallet_id = ?1 AND id = ?2 AND owner = ?3 AND phase = ?4
                 AND asset_lock_outpoint IS ?5",
            params![wallet_id, id, owner as i64, phase, outpoint],
        )? == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: &str = "aa";
    const TXID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn draft(label: &str) -> NewRegistration {
        NewRegistration {
            identity_index: None,
            identity: None,
            label: label.into(),
            temp_label: None,
            funding: r#"{"v":1,"kind":"core_balance","lock_duffs":1}"#.into(),
            initial_profile: None,
            phase: "draft".into(),
        }
    }

    fn outpoint(vout: u32) -> LockOutpoint {
        LockOutpoint::parse(&format!("{TXID}:{vout}")).unwrap()
    }

    #[test]
    fn outpoints_have_one_canonical_text() {
        let upper = format!("{}:7", TXID.to_ascii_uppercase());
        assert_eq!(
            LockOutpoint::parse(&upper).unwrap().as_str(),
            format!("{TXID}:7"),
            "upper-case hex is normalized"
        );
        for bad in [
            "T:0".to_string(),
            "t:0".to_string(),
            TXID.to_string(),
            format!("{TXID}:"),
            format!("{TXID}:01"),
            format!("{TXID}:+1"),
            format!("{TXID}: 1"),
            format!("{TXID}:-1"),
            format!("{TXID}:4294967296"),
            format!("{}:0", &TXID[1..]),
            format!("{TXID}0:0"),
            format!("{}g:0", &TXID[1..]),
            format!("{TXID}:0:0"),
        ] {
            assert_eq!(LockOutpoint::parse(&bad), None, "{bad:?}");
        }
        assert_eq!(
            LockOutpoint::parse(&format!("{TXID}:4294967295"))
                .unwrap()
                .as_str(),
            format!("{TXID}:4294967295")
        );
    }

    #[test]
    fn upper_case_input_cannot_make_a_second_flow_for_one_lock() {
        let db = AppDb::open_in_memory().unwrap();
        let a = db.insert_registration(W, &draft("alice"), 1).unwrap();
        let b = db.insert_registration(W, &draft("bob"), 1).unwrap();
        let lower = RegistrationChange {
            outpoint: Some(outpoint(0)),
            ..Default::default()
        };
        assert_eq!(
            db.update_registration(W, a, 0, "draft", false, &lower, 2)
                .unwrap(),
            RegistrationUpdate::Applied
        );
        let upper = RegistrationChange {
            outpoint: LockOutpoint::parse(&format!("{}:0", TXID.to_ascii_uppercase())),
            ..Default::default()
        };
        assert_eq!(
            db.update_registration(W, b, 0, "draft", false, &upper, 2)
                .unwrap(),
            RegistrationUpdate::LockTaken
        );
        // Another wallet may hold the same text.
        let c = db.insert_registration("bb", &draft("carol"), 1).unwrap();
        assert_eq!(
            db.update_registration("bb", c, 0, "draft", false, &lower, 2)
                .unwrap(),
            RegistrationUpdate::Applied
        );
        assert_eq!(
            db.registration_for_lock(W, outpoint(0).as_str()).unwrap(),
            Some(a)
        );
    }

    #[test]
    fn updates_are_compare_and_set_on_the_phase_and_failure() {
        let db = AppDb::open_in_memory().unwrap();
        let id = db.insert_registration(W, &draft("alice"), 1).unwrap();
        let to_keys = RegistrationChange {
            phase: Some("keys_prepared".into()),
            identity_index: Some(3),
            ..Default::default()
        };
        assert_eq!(
            db.update_registration(W, id, 0, "draft", false, &to_keys, 2)
                .unwrap(),
            RegistrationUpdate::Applied
        );
        // A second writer from the same phase loses.
        assert_eq!(
            db.update_registration(W, id, 0, "draft", false, &to_keys, 3)
                .unwrap(),
            RegistrationUpdate::Stale
        );
        let fail = RegistrationChange {
            failure: Some(Some(("platform.unavailable".into(), true))),
            ..Default::default()
        };
        assert_eq!(
            db.update_registration(W, id, 0, "keys_prepared", false, &fail, 4)
                .unwrap(),
            RegistrationUpdate::Applied
        );
        let row = db.registration_row(W, id).unwrap().unwrap();
        assert_eq!(
            (
                row.phase.as_str(),
                row.identity_index,
                row.error.as_deref(),
                row.retryable,
                row.updated_at
            ),
            (
                "keys_prepared",
                Some(3),
                Some("platform.unavailable"),
                true,
                4
            )
        );
        // Not failed any more? Then the unfailed CAS misses.
        assert_eq!(
            db.update_registration(W, id, 0, "keys_prepared", false, &to_keys, 5)
                .unwrap(),
            RegistrationUpdate::Stale
        );
        let resume = RegistrationChange {
            failure: Some(None),
            ..Default::default()
        };
        assert_eq!(
            db.update_registration(W, id, 0, "keys_prepared", true, &resume, 6)
                .unwrap(),
            RegistrationUpdate::Applied
        );
        let row = db.registration_row(W, id).unwrap().unwrap();
        assert_eq!((row.error, row.retryable), (None, false));
        assert!(
            db.delete_registration(W, id, 0, "keys_prepared", None)
                .unwrap()
        );
        assert!(
            !db.delete_registration(W, id, 0, "keys_prepared", None)
                .unwrap()
        );
        assert_eq!(
            db.update_registration(W, id, 0, "keys_prepared", false, &to_keys, 7)
                .unwrap(),
            RegistrationUpdate::Stale
        );
    }

    #[test]
    fn a_rejected_lock_leaves_the_flow_unfunded() {
        let db = AppDb::open_in_memory().unwrap();
        let id = db.insert_registration(W, &draft("alice"), 1).unwrap();
        let lock = LockOutpoint::parse(&format!("{TXID}:0")).unwrap();
        let funded = RegistrationChange {
            phase: Some("funding_sent".into()),
            outpoint: Some(lock.clone()),
            ..Default::default()
        };
        db.update_registration(W, id, 0, "draft", false, &funded, 2)
            .unwrap();
        let rejected = RegistrationChange {
            phase: Some("keys_prepared".into()),
            clear_outpoint: true,
            failure: Some(Some(("platform.broadcast_rejected".into(), true))),
            ..Default::default()
        };
        assert_eq!(
            db.update_registration(W, id, 0, "funding_sent", false, &rejected, 3)
                .unwrap(),
            RegistrationUpdate::Applied
        );
        let row = db.registration_row(W, id).unwrap().unwrap();
        assert_eq!(
            (row.phase.as_str(), row.asset_lock_outpoint, row.retryable),
            ("keys_prepared", None, true)
        );
        // The lock is free for another flow again.
        let other = db.insert_registration(W, &draft("bob"), 4).unwrap();
        assert_eq!(
            db.update_registration(W, other, 0, "draft", false, &funded, 5)
                .unwrap(),
            RegistrationUpdate::Applied
        );
    }

    #[test]
    fn adopting_a_lock_reuses_the_flow_that_holds_it() {
        let db = AppDb::open_in_memory().unwrap();
        let id = db.insert_registration(W, &draft("alice"), 1).unwrap();
        let held = RegistrationChange {
            phase: Some("funding_sent".into()),
            outpoint: Some(outpoint(1)),
            ..Default::default()
        };
        db.update_registration(W, id, 0, "draft", false, &held, 2)
            .unwrap();
        let adopted = NewRegistration {
            label: String::new(),
            phase: "funding_sent".into(),
            identity_index: Some(9),
            ..draft("")
        };
        assert_eq!(
            db.adopt_registration_lock(W, &outpoint(1), &adopted, 3)
                .unwrap(),
            Adoption::Existing(id)
        );
        let row = db.registration_row(W, id).unwrap().unwrap();
        assert_eq!((row.label.as_str(), row.updated_at), ("alice", 2));

        let Adoption::Inserted(other) = db
            .adopt_registration_lock(W, &outpoint(2), &adopted, 4)
            .unwrap()
        else {
            panic!("a new row for a new lock");
        };
        assert_ne!(other, id);
        assert_eq!(
            db.adopt_registration_lock(W, &outpoint(2), &adopted, 4)
                .unwrap(),
            Adoption::Existing(other)
        );
        assert_eq!(db.registration_rows(W).unwrap().len(), 2);
        // A third lock at the second's index: no row, the index is held.
        assert_eq!(
            db.adopt_registration_lock(W, &outpoint(3), &adopted, 5)
                .unwrap(),
            Adoption::IndexHeld
        );
        assert_eq!(db.registration_rows(W).unwrap().len(), 2);
    }

    /// DEC-171: a claim raises the row's owner; a write or delete of an
    /// earlier claim changes nothing, and a delete judged on a lock-less row
    /// keeps a row that got its lock since.
    #[test]
    fn only_the_latest_claim_writes() {
        let db = AppDb::open_in_memory().unwrap();
        let id = db.insert_registration(W, &draft("alice"), 1).unwrap();
        assert_eq!(db.registration_row(W, id).unwrap().unwrap().owner, 0);
        let old = db.claim_registration(W, id).unwrap().unwrap();
        let new = db.claim_registration(W, id).unwrap().unwrap();
        assert!(new > old);
        let to_keys = RegistrationChange {
            phase: Some("keys_prepared".into()),
            identity_index: Some(3),
            ..Default::default()
        };
        assert_eq!(
            db.update_registration(W, id, old, "draft", false, &to_keys, 2)
                .unwrap(),
            RegistrationUpdate::Stale
        );
        assert!(!db.delete_registration(W, id, old, "draft", None).unwrap());
        assert_eq!(
            db.update_registration(W, id, new, "draft", false, &to_keys, 2)
                .unwrap(),
            RegistrationUpdate::Applied
        );
        let lock = RegistrationChange {
            outpoint: Some(outpoint(4)),
            ..Default::default()
        };
        assert_eq!(
            db.update_registration(W, id, new, "keys_prepared", false, &lock, 3)
                .unwrap(),
            RegistrationUpdate::Applied
        );
        assert!(
            !db.delete_registration(W, id, new, "keys_prepared", None)
                .unwrap()
        );
        assert!(
            db.delete_registration(W, id, new, "keys_prepared", Some(outpoint(4).as_str()))
                .unwrap()
        );
        assert_eq!(db.claim_registration(W, id).unwrap(), None);
    }
}
