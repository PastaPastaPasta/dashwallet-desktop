//! The wallet's own governance proposals (`mg_proposals`, MG-06 §2.4, D4):
//! one row per proposal, saved before its fee transaction is broadcast and
//! carried by a `.dwbackup` (MG-00 D18). Every statement that writes the
//! table is here. The columns are the engine's facts; this crate does not
//! interpret `data` or `fee_tx`.

use rusqlite::{Row, params};

use crate::{AppDb, AppDbError, Result};

/// One `mg_proposals` row as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalRow {
    pub wallet_id: String,
    /// The object hash, wire order.
    pub hash: [u8; 32],
    pub revision: i32,
    /// The object's time, UNIX seconds.
    pub time: i64,
    /// The proposal JSON bytes.
    pub data: Vec<u8>,
    /// The fee transaction's txid, wire order.
    pub fee_txid: [u8; 32],
    /// The signed fee transaction, consensus bytes.
    pub fee_tx: Vec<u8>,
    /// When the row was saved, UNIX seconds.
    pub created_at: u64,
    /// When the hand-off of the fee was entered; `None` until then.
    pub fee_sent_at: Option<u64>,
    /// When a peer fetched the object; `None` until then.
    pub submitted_at: Option<u64>,
    pub last_error: Option<String>,
}

const COLUMNS: &str = "wallet_id, hash, revision, time, data, fee_txid, fee_tx, created_at,
     fee_sent_at, submitted_at, last_error";

fn corrupt(what: &str, value: impl std::fmt::Display) -> AppDbError {
    AppDbError::Corrupt(format!("mg_proposals.{what} {value}"))
}

fn to_db(what: &str, v: u64) -> Result<i64> {
    i64::try_from(v).map_err(|_| corrupt(what, v))
}

fn from_db(what: &str, v: i64) -> Result<u64> {
    u64::try_from(v).map_err(|_| corrupt(what, v))
}

fn key(what: &str, bytes: Vec<u8>) -> Result<[u8; 32]> {
    bytes
        .try_into()
        .map_err(|b: Vec<u8>| corrupt(what, format!("of {} bytes", b.len())))
}

/// One row, its values checked against their domains.
fn read_row(r: &Row<'_>) -> rusqlite::Result<Result<ProposalRow>> {
    let wallet_id: String = r.get(0)?;
    let hash: Vec<u8> = r.get(1)?;
    let revision: i64 = r.get(2)?;
    let time: i64 = r.get(3)?;
    let data: Vec<u8> = r.get(4)?;
    let fee_txid: Vec<u8> = r.get(5)?;
    let fee_tx: Vec<u8> = r.get(6)?;
    let created_at: i64 = r.get(7)?;
    let fee_sent_at: Option<i64> = r.get(8)?;
    let submitted_at: Option<i64> = r.get(9)?;
    let last_error: Option<String> = r.get(10)?;
    Ok((|| {
        Ok(ProposalRow {
            wallet_id,
            hash: key("hash", hash)?,
            revision: i32::try_from(revision).map_err(|_| corrupt("revision", revision))?,
            time,
            data,
            fee_txid: key("fee_txid", fee_txid)?,
            fee_tx,
            created_at: from_db("created_at", created_at)?,
            fee_sent_at: fee_sent_at.map(|v| from_db("fee_sent_at", v)).transpose()?,
            submitted_at: submitted_at
                .map(|v| from_db("submitted_at", v))
                .transpose()?,
            last_error,
        })
    })())
}

impl AppDb {
    /// Saves a proposal before its fee is broadcast, durably: committed
    /// under `synchronous=FULL`, since a row lost to a power cut after the
    /// broadcast burns the fee (MG-06 D4). A duplicate key is an error (the
    /// caller never saves one proposal twice); `WalletRemoved` once the
    /// wallet was removed (DEC-210).
    pub fn insert_proposal(&self, row: &ProposalRow) -> Result<()> {
        let created_at = to_db("created_at", row.created_at)?;
        let fee_sent_at = row
            .fee_sent_at
            .map(|v| to_db("fee_sent_at", v))
            .transpose()?;
        let submitted_at = row
            .submitted_at
            .map(|v| to_db("submitted_at", v))
            .transpose()?;
        self.durably(|conn| {
            conn.execute(
                &format!(
                    "INSERT INTO mg_proposals ({COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
                ),
                params![
                    row.wallet_id,
                    row.hash.as_slice(),
                    row.revision,
                    row.time,
                    row.data,
                    row.fee_txid.as_slice(),
                    row.fee_tx,
                    created_at,
                    fee_sent_at,
                    submitted_at,
                    row.last_error,
                ],
            )?;
            if conn.changes() == 0 {
                return Err(AppDbError::WalletRemoved(row.wallet_id.clone()));
            }
            Ok(())
        })
    }

    /// The proposals of `wallet_id`, or of every wallet if `None`, newest
    /// first (`created_at` descending, then `hash`).
    pub fn proposals(&self, wallet_id: Option<&str>) -> Result<Vec<ProposalRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM mg_proposals
             WHERE ?1 IS NULL OR wallet_id = ?1
             ORDER BY created_at DESC, hash, wallet_id"
        ))?;
        let rows = stmt
            .query_map(params![wallet_id], read_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().collect()
    }

    /// Records the hand-off of the fee at `at`, if none was recorded.
    /// Returns whether a row changed.
    pub fn mark_proposal_sent(&self, wallet_id: &str, hash: &[u8; 32], at: u64) -> Result<bool> {
        self.mark("fee_sent_at", wallet_id, hash, at)
    }

    /// Records that a peer fetched the object at `at`, if none was
    /// recorded. Returns whether a row changed.
    pub fn mark_proposal_submitted(
        &self,
        wallet_id: &str,
        hash: &[u8; 32],
        at: u64,
    ) -> Result<bool> {
        self.mark("submitted_at", wallet_id, hash, at)
    }

    fn mark(&self, column: &str, wallet_id: &str, hash: &[u8; 32], at: u64) -> Result<bool> {
        let at = to_db(column, at)?;
        Ok(self.conn().execute(
            &format!(
                "UPDATE mg_proposals SET {column} = ?3
                 WHERE wallet_id = ?1 AND hash = ?2 AND {column} IS NULL"
            ),
            params![wallet_id, hash.as_slice(), at],
        )? == 1)
    }

    /// Sets (or, with `None`, clears) the last failure's code. A missing
    /// row is not an error.
    pub fn set_proposal_error(
        &self,
        wallet_id: &str,
        hash: &[u8; 32],
        error: Option<&str>,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE mg_proposals SET last_error = ?3 WHERE wallet_id = ?1 AND hash = ?2",
            params![wallet_id, hash.as_slice(), error],
        )?;
        Ok(())
    }

    /// Keeps the proposals `hashes` of `wallet_id` past its removal (MG-06
    /// D23), durably, in place of what an earlier removal kept: its
    /// readmission ([`AppDb::readmit_wallet`]) copies them back, so a fee
    /// that may be out stays tied to its object.
    pub fn keep_proposals(&self, wallet_id: &str, hashes: &[[u8; 32]]) -> Result<()> {
        self.durably(|conn| {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "DELETE FROM mg_proposals_kept WHERE wallet_id = ?1",
                [wallet_id],
            )?;
            let copy = format!(
                "INSERT INTO mg_proposals_kept ({COLUMNS})
                 SELECT {COLUMNS} FROM mg_proposals WHERE wallet_id = ?1 AND hash = ?2"
            );
            for hash in hashes {
                tx.execute(&copy, params![wallet_id, hash.as_slice()])?;
            }
            Ok(tx.commit()?)
        })
    }

    /// Deletes the proposal `hash` paid by `fee_txid` whose fee was never
    /// handed off (a cancelled hand-off), and the copy a removal racing it
    /// kept (D23): a row with `fee_sent_at` set stays, checked in the same
    /// transaction (MG-06 D4), and so does another create's row of the same
    /// hash (its fee differs). Returns whether a row went.
    pub fn delete_unsent_proposal(
        &self,
        wallet_id: &str,
        hash: &[u8; 32],
        fee_txid: &[u8; 32],
    ) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut deleted = 0;
        for table in ["mg_proposals", "mg_proposals_kept"] {
            deleted += tx.execute(
                &format!(
                    "DELETE FROM {table} WHERE wallet_id = ?1 AND hash = ?2
                     AND fee_txid = ?3 AND fee_sent_at IS NULL"
                ),
                params![wallet_id, hash.as_slice(), fee_txid.as_slice()],
            )?;
        }
        tx.commit()?;
        Ok(deleted > 0)
    }
}

/// Copies back the proposals `wallet_id`'s removal kept, inside its
/// readmission. The copies stay: a readmission rolled back by a removal
/// finds them again.
pub(crate) fn restore_kept(
    tx: &rusqlite::Transaction<'_>,
    wallet_id: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO mg_proposals ({COLUMNS})
             SELECT {COLUMNS} FROM mg_proposals_kept WHERE wallet_id = ?1"
        ),
        [wallet_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: &str = "aa";

    /// The fee txid of `row(_, n, _)`.
    fn fee_of(n: u8) -> [u8; 32] {
        [n ^ 0xff; 32]
    }

    fn row(wallet: &str, n: u8, created_at: u64) -> ProposalRow {
        ProposalRow {
            wallet_id: wallet.into(),
            hash: [n; 32],
            revision: 1,
            time: 1_700_000_000 + i64::from(n),
            data: format!(r#"{{"name":"p{n}"}}"#).into_bytes(),
            fee_txid: fee_of(n),
            fee_tx: vec![n, 1, 2, 3],
            created_at,
            fee_sent_at: None,
            submitted_at: None,
            last_error: None,
        }
    }

    #[test]
    fn a_row_round_trips() {
        let db = AppDb::open_in_memory().unwrap();
        let mut r = row(W, 1, 10);
        r.last_error = Some("boom".into());
        db.insert_proposal(&r).unwrap();
        assert_eq!(db.proposals(Some(W)).unwrap(), vec![r.clone()]);
        assert!(db.proposals(Some("bb")).unwrap().is_empty());
        let full = ProposalRow {
            fee_sent_at: Some(11),
            submitted_at: Some(12),
            ..row(W, 2, 20)
        };
        db.insert_proposal(&full).unwrap();
        assert!(db.proposals(Some(W)).unwrap().contains(&full));
    }

    #[test]
    fn a_duplicate_key_is_an_error_and_the_bad_lengths_are_refused() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_proposal(&row(W, 1, 10)).unwrap();
        assert!(matches!(
            db.insert_proposal(&row(W, 1, 11)),
            Err(AppDbError::Sqlite(_))
        ));
        // The same hash under another wallet is another row.
        db.insert_proposal(&row("bb", 1, 11)).unwrap();
        let conn = db.conn();
        for (hash, txid) in [
            ("zeroblob(31)", "zeroblob(32)"),
            ("zeroblob(32)", "zeroblob(33)"),
        ] {
            assert!(
                conn.execute(
                    &format!(
                        "INSERT INTO mg_proposals (wallet_id, hash, revision, time, data,
                             fee_txid, fee_tx, created_at)
                         VALUES ('cc', {hash}, 1, 1, x'00', {txid}, x'00', 1)"
                    ),
                    [],
                )
                .is_err()
            );
        }
    }

    #[test]
    fn rows_come_newest_first_then_by_hash() {
        let db = AppDb::open_in_memory().unwrap();
        for r in [
            row(W, 3, 10),
            row(W, 1, 30),
            row(W, 2, 10),
            row("bb", 9, 20),
        ] {
            db.insert_proposal(&r).unwrap();
        }
        let order = |wallet| -> Vec<(String, u8)> {
            db.proposals(wallet)
                .unwrap()
                .into_iter()
                .map(|r| (r.wallet_id, r.hash[0]))
                .collect()
        };
        assert_eq!(
            order(Some(W)),
            [(W.into(), 1), (W.into(), 2), (W.into(), 3)]
        );
        assert_eq!(
            order(None),
            [
                (W.into(), 1),
                ("bb".into(), 9),
                (W.into(), 2),
                (W.into(), 3)
            ]
        );
    }

    #[test]
    fn the_marks_are_set_once() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_proposal(&row(W, 1, 10)).unwrap();
        let h = [1; 32];
        assert!(db.mark_proposal_sent(W, &h, 11).unwrap());
        assert!(!db.mark_proposal_sent(W, &h, 99).unwrap());
        assert!(db.mark_proposal_submitted(W, &h, 12).unwrap());
        assert!(!db.mark_proposal_submitted(W, &h, 99).unwrap());
        assert!(!db.mark_proposal_sent(W, &[2; 32], 11).unwrap());
        assert!(!db.mark_proposal_sent("bb", &h, 11).unwrap());
        let r = &db.proposals(Some(W)).unwrap()[0];
        assert_eq!((r.fee_sent_at, r.submitted_at), (Some(11), Some(12)));
    }

    #[test]
    fn the_error_is_set_and_cleared() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_proposal(&row(W, 1, 10)).unwrap();
        let h = [1; 32];
        let error = |db: &AppDb| db.proposals(Some(W)).unwrap()[0].last_error.clone();
        db.set_proposal_error(W, &h, Some("no_peer")).unwrap();
        assert_eq!(error(&db).as_deref(), Some("no_peer"));
        db.set_proposal_error(W, &h, None).unwrap();
        assert_eq!(error(&db), None);
        db.set_proposal_error(W, &[9; 32], Some("x")).unwrap();
        assert_eq!(error(&db), None);
    }

    #[test]
    fn a_proposal_is_deleted_once() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_proposal(&row(W, 1, 10)).unwrap();
        db.insert_proposal(&row(W, 2, 10)).unwrap();
        assert!(db.delete_unsent_proposal(W, &[1; 32], &fee_of(1)).unwrap());
        assert!(!db.delete_unsent_proposal(W, &[1; 32], &fee_of(1)).unwrap());
        assert!(
            !db.delete_unsent_proposal("bb", &[2; 32], &fee_of(2))
                .unwrap()
        );
        assert_eq!(db.proposals(None).unwrap().len(), 1);
    }

    /// Review r1 M-1: a fee a resend handed out keeps its proposal.
    #[test]
    fn a_proposal_whose_fee_went_out_is_not_deleted() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_proposal(&row(W, 1, 10)).unwrap();
        assert!(db.mark_proposal_sent(W, &[1; 32], 11).unwrap());
        assert!(!db.delete_unsent_proposal(W, &[1; 32], &fee_of(1)).unwrap());
        assert_eq!(db.proposals(Some(W)).unwrap()[0].fee_sent_at, Some(11));
    }

    /// Review r1 validator Minor-4/-3: a cancelled hand-off deletes only its
    /// own row (another create of the same hash pays another fee), and the
    /// copy a racing removal kept of it.
    #[test]
    fn a_cancelled_hand_off_deletes_only_its_own_row_and_its_kept_copy() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_proposal(&row(W, 1, 10)).unwrap();
        assert!(!db.delete_unsent_proposal(W, &[1; 32], &[7; 32]).unwrap());
        assert_eq!(db.proposals(Some(W)).unwrap().len(), 1);
        db.keep_proposals(W, &[[1; 32]]).unwrap();
        assert!(db.delete_unsent_proposal(W, &[1; 32], &fee_of(1)).unwrap());
        assert!(db.proposals(Some(W)).unwrap().is_empty());
        let kept: i64 = (db.conn())
            .query_row("SELECT COUNT(*) FROM mg_proposals_kept", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, 0, "the kept copy goes with it");
    }

    /// D4 (review r1 H2): the save survives a power cut right after it,
    /// with nothing written since to sync it.
    #[test]
    fn a_saved_proposal_survives_power_loss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(crate::APP_DB_FILE);
        drop(AppDb::open(&path).unwrap());
        crate::durability_sim::arm(&path, false);
        let db = AppDb::open(&path).unwrap();
        db.insert_proposal(&row(W, 1, 10)).unwrap();
        drop(db);
        crate::durability_sim::power_loss(&path);
        let db = AppDb::open(&path).unwrap();
        assert_eq!(db.proposals(Some(W)).unwrap(), vec![row(W, 1, 10)]);
    }

    /// DEC-210: `delete_wallet` takes the rows, and a later save of the
    /// wallet is refused rather than ignored.
    #[test]
    fn a_removed_wallet_keeps_no_proposal_and_gets_none() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_proposal(&row(W, 1, 10)).unwrap();
        db.insert_proposal(&row("bb", 1, 10)).unwrap();
        db.delete_wallet(W).unwrap();
        assert!(db.proposals(Some(W)).unwrap().is_empty());
        assert_eq!(db.proposals(Some("bb")).unwrap().len(), 1);
        assert!(matches!(
            db.insert_proposal(&row(W, 2, 11)),
            Err(AppDbError::WalletRemoved(w)) if w == W
        ));
        assert!(db.proposals(Some(W)).unwrap().is_empty());
    }

    /// D23: what a removal keeps comes back with the wallet's readmission,
    /// whether the removal record is there or was forgotten, and a later
    /// removal's choice replaces it.
    #[test]
    fn a_removal_keeps_the_chosen_proposals_for_the_readmission() {
        let db = AppDb::open_in_memory().unwrap();
        for n in 1..=3 {
            db.insert_proposal(&row(W, n, 10)).unwrap();
        }
        db.insert_proposal(&row("bb", 1, 10)).unwrap();
        db.keep_proposals(W, &[[1; 32], [3; 32]]).unwrap();
        db.delete_wallet(W).unwrap();
        assert!(db.proposals(Some(W)).unwrap().is_empty());
        assert!(db.export_wallet_rows(W).unwrap().is_empty(), "not carried");
        assert!(db.readmit_wallet(W).unwrap());
        let hashes = |db: &AppDb| -> Vec<u8> {
            db.proposals(Some(W))
                .unwrap()
                .iter()
                .map(|r| r.hash[0])
                .collect()
        };
        assert_eq!(hashes(&db), [1, 3]);
        assert_eq!(db.proposals(Some("bb")).unwrap().len(), 1);

        db.keep_proposals(W, &[[3; 32]]).unwrap();
        db.delete_wallet(W).unwrap();
        // A record forgotten (a lone open): the readmission is no
        // readmission, and the rows still come back.
        db.conn()
            .execute("DELETE FROM removed_wallets", [])
            .unwrap();
        assert!(!db.readmit_wallet(W).unwrap());
        assert_eq!(hashes(&db), [3]);
    }

    /// A `.dwbackup` carries the table: the fee is 1 DASH (MG-00 D18).
    #[test]
    fn a_backup_round_trip_restores_the_rows() {
        let a = AppDb::open_in_memory().unwrap();
        let sent = ProposalRow {
            fee_sent_at: Some(11),
            last_error: Some("e".into()),
            ..row(W, 1, 10)
        };
        a.insert_proposal(&sent).unwrap();
        a.insert_proposal(&row(W, 2, 20)).unwrap();
        a.insert_proposal(&row("bb", 3, 30)).unwrap();
        let rows = a.export_wallet_rows(W).unwrap();
        let table = rows.iter().find(|t| t.table == "mg_proposals").unwrap();
        assert_eq!(table.rows.len(), 2);

        let b = AppDb::open_in_memory().unwrap();
        assert_eq!(
            b.import_wallet_rows(W, &rows).unwrap(),
            rows.iter().map(|t| t.rows.len()).sum::<usize>()
        );
        assert_eq!(b.proposals(None).unwrap(), a.proposals(Some(W)).unwrap());
        assert_eq!(b.import_wallet_rows(W, &rows).unwrap(), 0);
    }
}
