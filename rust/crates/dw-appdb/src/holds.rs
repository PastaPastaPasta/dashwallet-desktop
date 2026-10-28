//! The coins a masternode row holds (MG-04b BL-346, M3): an MG-04
//! operation's `fee_inputs` until they settle, a shared session's
//! `reserved` until it is released. Read from the stored columns, so a row
//! whose envelope or terms no longer parse holds what it stored. A coin a
//! row holds stays locked whoever unlocks it: a row's own release skips
//! what another row holds, and the user's unlock is refused (the engine's
//! `unlock_outpoints`).

use std::collections::HashSet;

use rusqlite::{Connection, params};

use crate::{AppDb, Result};

/// The row a release is made for, whose own hold doesn't count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder<'a> {
    /// The user, or nothing: every row's hold counts.
    None,
    /// An MG-04 operation, by id.
    Operation(&'a str),
    /// A shared session, by id.
    Session(&'a str),
}

impl<'a> Holder<'a> {
    fn operation(self) -> Option<&'a str> {
        match self {
            Holder::Operation(id) => Some(id),
            _ => None,
        }
    }

    fn session(self) -> Option<&'a str> {
        match self {
            Holder::Session(id) => Some(id),
            _ => None,
        }
    }
}

/// Every `"txid:vout"` a row of `wallet_id` other than `except` holds, as
/// one query: the rows' JSON arrays, any other column read as empty.
const HELD: &str = "
    SELECT j.value FROM shared_mn_sessions s,
        json_each(CASE WHEN json_valid(s.reserved) AND json_type(s.reserved) = 'array'
            THEN s.reserved ELSE '[]' END) j
    WHERE s.wallet_id = ?1 AND s.id IS NOT ?2 AND j.type = 'text'
    UNION
    SELECT j.value FROM masternode_ops o,
        json_each(CASE WHEN json_valid(o.fee_inputs) AND json_type(o.fee_inputs) = 'array'
            THEN o.fee_inputs ELSE '[]' END) j
    WHERE o.wallet_id = ?1 AND o.id IS NOT ?3 AND j.type = 'text'";

fn held_on(conn: &Connection, wallet_id: &str, except: Holder<'_>) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(HELD)?;
    let rows = stmt.query_map(
        params![wallet_id, except.session(), except.operation()],
        |r| r.get::<_, String>(0),
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

impl AppDb {
    /// The `(txid, vout)` every masternode row of the wallet holds but
    /// `except`.
    pub fn held_outpoints(
        &self,
        wallet_id: &str,
        except: Holder<'_>,
    ) -> Result<HashSet<(String, u32)>> {
        Ok(held_on(&self.conn(), wallet_id, except)?
            .iter()
            .filter_map(|item| {
                let (txid, vout) = item.rsplit_once(':')?;
                Some((txid.to_owned(), vout.parse().ok()?))
            })
            .collect())
    }

    /// [`Self::unlock`] for `holder`'s release, skipping what another row
    /// holds. Returns whether a lock was active and released.
    pub fn release_lock(
        &self,
        wallet_id: &str,
        holder: Holder<'_>,
        txid: &str,
        vout: u32,
        now: u64,
    ) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let released = Self::release_lock_on(&tx, wallet_id, holder, txid, vout, now)?;
        tx.commit()?;
        Ok(released)
    }

    /// [`Self::release_lock`] inside a caller's transaction.
    pub(crate) fn release_lock_on(
        conn: &Connection,
        wallet_id: &str,
        holder: Holder<'_>,
        txid: &str,
        vout: u32,
        now: u64,
    ) -> Result<bool> {
        if held_on(conn, wallet_id, holder)?.contains(&format!("{txid}:{vout}")) {
            return Ok(false);
        }
        Self::unlock_on(conn, wallet_id, txid, vout, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MasternodeOpRow, SharedSessionRow};

    const W: &str = "aa";
    const S1: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    fn session(id: &str, reserved: &str) -> SharedSessionRow {
        SharedSessionRow {
            id: id.into(),
            kind: "registration".into(),
            role: "coordinator".into(),
            envelope: "{}".into(),
            my_shares: "{}".into(),
            contribution: None,
            reserved: reserved.into(),
            approved_terms: None,
            base_keys: None,
            signed_tx: None,
            txid: None,
            out_at: None,
            abandon_txid: None,
            created_at: 1,
            updated_at: 1,
            last_error: None,
        }
    }

    fn operation(id: &str, fee_inputs: &str) -> MasternodeOpRow {
        MasternodeOpRow {
            id: id.into(),
            kind: "registration".into(),
            terms: "{}".into(),
            request: "{}".into(),
            unsigned_tx: None,
            sign_address: None,
            sign_message: None,
            fee_inputs: fee_inputs.into(),
            signed_tx: Some(vec![1]),
            txid: Some("t".into()),
            pro_tx_hash: None,
            locked_collateral: false,
            fee: 1,
            created_at: 1,
            signed_at: Some(1),
            sent_at: None,
            last_error: None,
        }
    }

    fn locked(db: &AppDb) -> HashSet<(String, u32)> {
        (db.locks(W).unwrap().into_iter())
            .filter(|r| r.is_locked())
            .map(|r| (r.txid, r.vout))
            .collect()
    }

    fn pair(txid: &str, vout: u32) -> (String, u32) {
        (txid.into(), vout)
    }

    /// Every row's coins count but `except`'s; a column that isn't a JSON
    /// array of strings holds nothing.
    #[test]
    fn held_outpoints_read_both_tables() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_shared_session_durable(W, &session(S1, r#"["a:0","b:1"]"#), 1)
            .unwrap();
        db.insert_masternode_op_durable(W, &operation("o1", r#"["c:2"]"#), &[], 1)
            .unwrap();
        db.insert_masternode_op_durable(W, &operation("o2", "not json"), &[], 1)
            .unwrap();
        db.insert_masternode_op_durable(W, &operation("o3", "[7]"), &[], 1)
            .unwrap();
        db.insert_masternode_op_durable(W, &operation("o4", r#"{"k":"d:3"}"#), &[], 1)
            .unwrap();
        db.insert_masternode_op_durable(W, &operation("o5", r#""e:4""#), &[], 1)
            .unwrap();
        let all = db.held_outpoints(W, Holder::None).unwrap();
        assert_eq!(
            all,
            HashSet::from([pair("a", 0), pair("b", 1), pair("c", 2)])
        );
        assert_eq!(
            db.held_outpoints(W, Holder::Session(S1)).unwrap(),
            HashSet::from([pair("c", 2)])
        );
        assert_eq!(
            db.held_outpoints(W, Holder::Operation("o1")).unwrap(),
            HashSet::from([pair("a", 0), pair("b", 1)])
        );
        assert!(db.held_outpoints("bb", Holder::None).unwrap().is_empty());
    }

    /// A row's release skips what another row holds; a session's discard
    /// and an operation's release alike.
    #[test]
    fn a_release_never_frees_another_rows_coin() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_shared_session_durable(W, &session(S1, r#"["a:0","b:1"]"#), 1)
            .unwrap();
        db.insert_masternode_op_durable(W, &operation("o1", r#"["a:0"]"#), &[pair("a", 0)], 1)
            .unwrap();
        assert!(
            !db.release_lock(W, Holder::Operation("o1"), "a", 0, 2)
                .unwrap()
        );
        assert!(locked(&db).contains(&pair("a", 0)));
        // The session's discard keeps `a:0` (the operation holds it) and
        // frees `b:1`.
        assert!(db.delete_unsent_shared_session_durable(W, S1, 3).unwrap());
        assert_eq!(locked(&db), HashSet::from([pair("a", 0)]));
        // Now the operation alone holds it: its release frees it.
        assert!(
            db.release_lock(W, Holder::Operation("o1"), "a", 0, 4)
                .unwrap()
        );
        assert!(locked(&db).is_empty());
    }
}
