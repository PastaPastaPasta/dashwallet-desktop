//! MG-13 §3's storage tests: P6's table through the compare-and-set writes,
//! the unique index across connections, P15's one transaction, the group
//! floor, P16's barriers with their operation, the wallet removal, and every
//! fault point.

use std::path::{Path, PathBuf};

use super::*;
use crate::fault::{self, Stage};
use crate::{AppDb, AppDbError, MasternodeOpRow, NewPayoutBarrier};

use WithdrawalState::*;

const W: &str = "aa";
const W2: &str = "bb";
const IDENTITY: &str = "ident-1";
const PRO_TX: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const PRO_TX2: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const TXID: &str = "3333333333333333333333333333333333333333333333333333333333333333";

/// A record of `IDENTITY` at `nonce`, signed by the owner key.
fn signed(id: &str, nonce: u64) -> SignedWithdrawal {
    let hash: String = id.bytes().map(|b| format!("{b:02x}")).collect();
    SignedWithdrawal {
        id: id.into(),
        transition_hash: format!("{hash:0>64}"),
        created_by: W.into(),
        pro_tx_hash: PRO_TX.into(),
        identity_id: IDENTITY.into(),
        key_kind: WithdrawalKeyKind::Owner,
        amount: 1_000_000,
        core_fee_per_byte: 1,
        pooling: 0,
        expected_duffs: 1_000,
        nonce,
        signed_bytes: id.as_bytes().to_vec(),
        script: None,
        expected_script: Some(vec![0x76, 0xa9]),
        signed_at: 10,
    }
}

fn proof() -> WithdrawalProof {
    WithdrawalProof {
        document_id: "doc".into(),
        height: 50,
        block_time: 1_700_000_000,
        paid_script: vec![0x76, 0xa9],
        document_status: 0,
    }
}

fn evidence() -> NotExecutedEvidence {
    NotExecutedEvidence {
        nonce_height: 40,
        identity_height: 41,
        document_height: 42,
        reason: Some("nonce used".into()),
    }
}

fn state(db: &AppDb, id: &str) -> WithdrawalState {
    db.withdrawal(id).unwrap().unwrap().state
}

/// A database file in a fresh directory (fault points are armed per file).
fn file_db() -> (tempfile::TempDir, PathBuf, AppDb) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    let db = AppDb::open(&path).unwrap();
    (dir, path, db)
}

#[test]
fn a_record_is_written_signed_and_read_back() {
    let db = AppDb::open_in_memory().unwrap();
    let mut row = signed("a", 3);
    row.key_kind = WithdrawalKeyKind::Payout;
    row.script = Some(vec![1, 2]);
    row.expected_script = None;
    db.insert_withdrawal_durable(&row, 7).unwrap();
    let read = db.withdrawal("a").unwrap().unwrap();
    assert_eq!(read.signed, row);
    assert_eq!(
        (
            read.state,
            read.sent_at,
            read.floor_height,
            read.proof,
            read.mark
        ),
        (Signed, None, 7, None, None)
    );
    assert_eq!(
        db.open_withdrawal(IDENTITY).unwrap().unwrap().signed.id,
        "a"
    );
    assert_eq!(db.withdrawals_of_identity(IDENTITY).unwrap().len(), 1);
    assert_eq!(db.withdrawals_of_masternode(PRO_TX).unwrap().len(), 1);
    assert!(db.withdrawals_of_masternode(PRO_TX2).unwrap().is_empty());
    assert_eq!(db.withdrawal_group(IDENTITY, 3).unwrap().len(), 1);
    assert!(db.withdrawal_group(IDENTITY, 4).unwrap().is_empty());
    assert_eq!(db.undecided_withdrawals().unwrap().len(), 1);
    assert!(db.withdrawal("b").unwrap().is_none());

    // The schema's domain: each key kind's script.
    let mut owner_with_script = signed("b", 4);
    owner_with_script.identity_id = "other".into();
    owner_with_script.script = Some(vec![1]);
    assert!(matches!(
        db.insert_withdrawal_durable(&owner_with_script, 0),
        Err(AppDbError::Sqlite(_))
    ));
}

/// What was signed, `sent_at` once set, and the row itself never change.
#[test]
fn signed_fields_sent_at_and_rows_are_kept() {
    let db = AppDb::open_in_memory().unwrap();
    db.insert_withdrawal_durable(&signed("a", 3), 0).unwrap();
    let conn = db.conn();
    for set in [
        "signed_bytes = x'00'",
        "nonce = 4",
        "amount = 5",
        "identity_id = 'x'",
        "script = x'01'",
        "expected_script = NULL",
    ] {
        let e = conn
            .execute(&format!("UPDATE mn_withdrawals SET {set}"), [])
            .unwrap_err();
        assert!(e.to_string().contains("not changed"), "{set}: {e}");
    }
    conn.execute("UPDATE mn_withdrawals SET sent_at = 5", [])
        .unwrap();
    assert!(
        conn.execute("UPDATE mn_withdrawals SET sent_at = 6", [])
            .is_err()
    );
    assert!(
        conn.execute("UPDATE mn_withdrawals SET sent_at = NULL", [])
            .is_err()
    );
    assert!(conn.execute("DELETE FROM mn_withdrawals", []).is_err());
}

#[test]
fn sent_at_is_set_once_and_reports_an_earlier_send() {
    let db = AppDb::open_in_memory().unwrap();
    assert_eq!(db.begin_withdrawal_send_durable("a", 5).unwrap(), None);
    db.insert_withdrawal_durable(&signed("a", 3), 0).unwrap();
    assert_eq!(
        db.begin_withdrawal_send_durable("a", 5).unwrap(),
        Some(false)
    );
    assert_eq!(
        db.begin_withdrawal_send_durable("a", 9).unwrap(),
        Some(true)
    );
    assert_eq!(db.withdrawal("a").unwrap().unwrap().sent_at, Some(5));
}

/// P6's table, written out here independently of `may_become`.
fn p6_allows(from: WithdrawalState, to: WithdrawalState) -> bool {
    let to_final = !from.is_final() && matches!(to, Done | SiblingPaid | NotExecuted);
    let listed = match from {
        Signed => [Sent, Refused, CouldntConfirm].contains(&to),
        Sent => [Refused, CouldntConfirm].contains(&to),
        Refused => [Superseded, CouldntConfirm].contains(&to),
        CouldntConfirm => [MarkedReceived, MarkedNotReceived].contains(&to),
        _ => false,
    };
    listed || to_final
}

/// A group of `IDENTITY` at nonce 3 whose record "x" is in `from`, with an
/// older sibling "o" (`superseded` while the group is undecided), and for
/// `from = superseded` an open sibling "y".
fn group_with_x_in(from: WithdrawalState) -> AppDb {
    let db = AppDb::open_in_memory().unwrap();
    db.insert_withdrawal_durable(&signed("o", 3), 0).unwrap();
    assert!(
        db.set_withdrawal_state_durable("o", &[Signed], Refused, Some("r"))
            .unwrap()
    );
    assert!(
        db.supersede_and_insert_withdrawal_durable("o", &signed("x", 3), 0)
            .unwrap()
    );
    let to = |db: &AppDb, from, to| {
        assert!(
            db.set_withdrawal_state_durable("x", &[from], to, None)
                .unwrap()
        )
    };
    match from {
        Signed => {}
        Sent | Refused | CouldntConfirm => to(&db, Signed, from),
        Superseded => {
            to(&db, Signed, Refused);
            assert!(
                db.supersede_and_insert_withdrawal_durable("x", &signed("y", 3), 0)
                    .unwrap()
            );
        }
        MarkedReceived | MarkedNotReceived => {
            to(&db, Signed, CouldntConfirm);
            let mark = if from == MarkedReceived {
                WithdrawalMark::Received
            } else {
                WithdrawalMark::NotReceived
            };
            assert!(db.mark_withdrawal_durable("x", mark, 9).unwrap());
        }
        Done => assert!(
            db.finalize_withdrawal_group_durable("x", &[Signed], &proof())
                .unwrap()
        ),
        SiblingPaid => assert!(
            db.finalize_withdrawal_group_durable("o", &[Superseded], &proof())
                .unwrap()
        ),
        NotExecuted => assert!(
            db.not_executed_withdrawal_group_durable(
                IDENTITY,
                3,
                &[("o", Superseded), ("x", Signed)],
                &evidence()
            )
            .unwrap()
        ),
    }
    assert_eq!(state(&db, "x"), from);
    db
}

/// Moves "x" from `from` to `to` through the write that makes that change.
/// Whether it applied (an `Err` is a refusal with nothing written).
fn apply(db: &AppDb, from: WithdrawalState, to: WithdrawalState) -> bool {
    let applied = match to {
        Signed => return false, // only an insert writes `signed`
        Sent | Refused | CouldntConfirm => db.set_withdrawal_state_durable("x", &[from], to, None),
        Superseded => db.supersede_and_insert_withdrawal_durable("x", &signed("z", 3), 0),
        MarkedReceived => db.mark_withdrawal_durable("x", WithdrawalMark::Received, 9),
        MarkedNotReceived => db.mark_withdrawal_durable("x", WithdrawalMark::NotReceived, 9),
        Done => db.finalize_withdrawal_group_durable("x", &[from], &proof()),
        SiblingPaid => {
            // Another record of the group is done.
            let other = if from == Superseded { "y" } else { "o" };
            let other_state = state(db, other);
            let _ = db.finalize_withdrawal_group_durable(other, &[other_state], &proof());
            return state(db, "x") == SiblingPaid && from != SiblingPaid;
        }
        NotExecuted => {
            let group = db.withdrawal_group(IDENTITY, 3).unwrap();
            let expected: Vec<_> = (group.iter())
                .map(|r| (r.signed.id.as_str(), r.state))
                .collect();
            db.not_executed_withdrawal_group_durable(IDENTITY, 3, &expected, &evidence())
        }
    };
    match applied {
        Ok(applied) => applied,
        Err(AppDbError::TransitionRefused { .. }) => false,
        Err(e) => panic!("{from:?} -> {to:?}: {e}"),
    }
}

/// MG-13 §3 P6: every pair of states, through the writes. Exactly P6's
/// table applies; nothing else changes the record.
#[test]
fn every_transition_of_p6_applies_and_no_other() {
    for from in WithdrawalState::ALL {
        for to in WithdrawalState::ALL {
            assert_eq!(
                from.may_become(to),
                p6_allows(from, to),
                "may_become {from:?} -> {to:?}"
            );
            let db = group_with_x_in(from);
            let applied = apply(&db, from, to);
            assert_eq!(applied, p6_allows(from, to), "{from:?} -> {to:?}");
            let after = state(&db, "x");
            assert_eq!(after, if applied { to } else { from }, "{from:?} -> {to:?}");
            if !applied {
                assert!(
                    db.withdrawal("z").unwrap().is_none(),
                    "{from:?} -> {to:?}: no sibling"
                );
            }
        }
    }
}

/// The schema's trigger is the same table for any writer.
#[test]
fn the_schema_refuses_every_transition_outside_p6() {
    for from in WithdrawalState::ALL {
        for to in WithdrawalState::ALL {
            if from == to {
                continue;
            }
            let db = group_with_x_in(from);
            let raw = db.conn().execute(
                "UPDATE mn_withdrawals SET state = ?1, proof_document_id = 'd', proof_height = 1,
                     paid_script = x'00', evidence_nonce_height = 1,
                     evidence_identity_height = 1, evidence_document_height = 1,
                     user_mark = 'received', marked_at = 1
                 WHERE id = 'x'",
                [to.as_str()],
            );
            match raw {
                Ok(_) => assert!(p6_allows(from, to), "{from:?} -> {to:?} written"),
                Err(e) => {
                    assert!(!p6_allows(from, to), "{from:?} -> {to:?}: {e}");
                    assert!(e.to_string().contains("not in MG-13 P6"), "{e}");
                }
            }
        }
    }
}

/// A non-open record never re-enters the open set, a final one is never
/// left, and the single-record write takes only its own targets.
#[test]
fn the_state_write_refuses_reentry_and_other_methods_targets() {
    let db = group_with_x_in(Superseded);
    for to in [Signed, Sent, Refused, CouldntConfirm] {
        assert!(matches!(
            db.set_withdrawal_state_durable("x", &[Superseded], to, None),
            Err(AppDbError::TransitionRefused { .. })
        ));
    }
    let db = group_with_x_in(Refused);
    for to in [Superseded, Done, SiblingPaid, NotExecuted, MarkedReceived] {
        assert!(matches!(
            db.set_withdrawal_state_durable("x", &[Refused, CouldntConfirm], to, None),
            Err(AppDbError::TransitionRefused { .. })
        ));
    }
    assert_eq!(state(&db, "x"), Refused);
    // A `from` the record isn't in: no change.
    assert!(
        !db.set_withdrawal_state_durable("x", &[Signed, Sent], CouldntConfirm, None)
            .unwrap()
    );
    assert!(
        db.set_withdrawal_state_durable("x", &[Signed, Sent, Refused], CouldntConfirm, Some("u"))
            .unwrap()
    );
    let x = db.withdrawal("x").unwrap().unwrap();
    assert_eq!(
        (x.state, x.state_reason.as_deref()),
        (CouldntConfirm, Some("u"))
    );
}

/// MG-13 §3 P6: a mark racing a proof. Whichever lands first, the other's
/// compare-and-set (against what it read) does not apply; a late proof then
/// applies over the mark, which is kept apart.
#[test]
fn a_mark_racing_a_proof_applies_once() {
    // The proof first.
    let db = group_with_x_in(CouldntConfirm);
    assert!(
        db.finalize_withdrawal_group_durable("x", &[CouldntConfirm], &proof())
            .unwrap()
    );
    assert!(
        !db.mark_withdrawal_durable("x", WithdrawalMark::NotReceived, 9)
            .unwrap()
    );
    let x = db.withdrawal("x").unwrap().unwrap();
    assert_eq!((x.state, x.mark), (Done, None));

    // The mark first.
    let db = group_with_x_in(CouldntConfirm);
    assert!(
        db.mark_withdrawal_durable("x", WithdrawalMark::NotReceived, 9)
            .unwrap()
    );
    assert!(
        !db.finalize_withdrawal_group_durable("x", &[CouldntConfirm], &proof())
            .unwrap()
    );
    assert_eq!(state(&db, "x"), MarkedNotReceived);
    // Re-read: the late proof (P9).
    assert!(
        db.finalize_withdrawal_group_durable("x", &[MarkedNotReceived], &proof())
            .unwrap()
    );
    let x = db.withdrawal("x").unwrap().unwrap();
    assert_eq!(x.state, Done);
    assert_eq!(x.mark, Some((WithdrawalMark::NotReceived, 9)), "kept apart");
    assert_eq!(
        x.proof,
        Some(ProvedDocument {
            document_id: "doc".into(),
            height: 50,
            block_time: Some(1_700_000_000)
        })
    );
    assert_eq!(
        (x.paid_script, x.document_status),
        (Some(vec![0x76, 0xa9]), Some(0))
    );
    assert_eq!(state(&db, "o"), SiblingPaid);
}

/// MG-13 §3 P6: two rechecks of one group read it alike and write different
/// outcomes; exactly one applies, in either order, and on two connections
/// racing.
#[test]
fn two_rechecks_writing_different_outcomes_apply_once() {
    let read = [("o", Superseded), ("x", Sent)];
    let db = group_with_x_in(Sent);
    assert!(
        db.finalize_withdrawal_group_durable("x", &[Sent], &proof())
            .unwrap()
    );
    assert!(
        !db.not_executed_withdrawal_group_durable(IDENTITY, 3, &read, &evidence())
            .unwrap()
    );
    assert_eq!((state(&db, "o"), state(&db, "x")), (SiblingPaid, Done));

    let db = group_with_x_in(Sent);
    assert!(
        db.not_executed_withdrawal_group_durable(IDENTITY, 3, &read, &evidence())
            .unwrap()
    );
    assert!(
        !db.finalize_withdrawal_group_durable("x", &[Sent], &proof())
            .unwrap()
    );
    let x = db.withdrawal("x").unwrap().unwrap();
    assert_eq!(x.state, NotExecuted);
    assert_eq!(x.evidence, Some(evidence()));
    assert_eq!(state(&db, "o"), NotExecuted);

    for _ in 0..8 {
        let (_dir, path, db) = file_db();
        db.insert_withdrawal_durable(&signed("x", 3), 0).unwrap();
        let other = AppDb::open(&path).unwrap();
        let start = std::sync::Barrier::new(2);
        let (a, b) = std::thread::scope(|s| {
            let a = s.spawn(|| {
                start.wait();
                db.finalize_withdrawal_group_durable("x", &[Signed], &proof())
                    .unwrap()
            });
            let b = s.spawn(|| {
                start.wait();
                other
                    .not_executed_withdrawal_group_durable(
                        IDENTITY,
                        3,
                        &[("x", Signed)],
                        &evidence(),
                    )
                    .unwrap()
            });
            (a.join().unwrap(), b.join().unwrap())
        });
        assert!(a ^ b, "exactly one applies: {a} {b}");
        assert_eq!(state(&db, "x"), if a { Done } else { NotExecuted });
    }
}

#[test]
fn a_group_decision_needs_the_group_as_read() {
    let db = group_with_x_in(Refused);
    // A sibling the caller didn't see.
    assert!(
        !db.not_executed_withdrawal_group_durable(IDENTITY, 3, &[("x", Refused)], &evidence())
            .unwrap()
    );
    // A state the caller didn't see.
    assert!(
        !db.not_executed_withdrawal_group_durable(
            IDENTITY,
            3,
            &[("o", Superseded), ("x", Sent)],
            &evidence()
        )
        .unwrap()
    );
    assert!(matches!(
        db.not_executed_withdrawal_group_durable(IDENTITY, 3, &[], &evidence()),
        Err(AppDbError::Invalid(_))
    ));
    assert_eq!((state(&db, "o"), state(&db, "x")), (Superseded, Refused));
    // Another group is untouched by a decision.
    db.finalize_withdrawal_group_durable("x", &[Refused], &proof())
        .unwrap();
    let mut other = signed("n", 4);
    other.identity_id = "ident-2".into();
    db.insert_withdrawal_durable(&other, 0).unwrap();
    assert_eq!(state(&db, "n"), Signed);
    assert_eq!(db.undecided_withdrawals().unwrap().len(), 1);
}

/// MG-13 §3: the unique index on its own, two connections to one file
/// racing an insert for one identity: exactly one wins, the other gets
/// `WithdrawalPending`.
#[test]
fn the_unique_index_lets_one_racing_insert_win() {
    for round in 0..8 {
        let (_dir, path, db) = file_db();
        let other = AppDb::open(&path).unwrap();
        let start = std::sync::Barrier::new(2);
        let results = std::thread::scope(|s| {
            let a = s.spawn(|| {
                start.wait();
                db.insert_withdrawal_durable(&signed("a", round), 0)
            });
            let b = s.spawn(|| {
                start.wait();
                other.insert_withdrawal_durable(&signed("b", round + 1), 0)
            });
            [a.join().unwrap(), b.join().unwrap()]
        });
        let won = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(won, 1, "{results:?}");
        assert!(
            results
                .iter()
                .any(|r| matches!(r, Err(AppDbError::WithdrawalPending(i)) if i == IDENTITY)),
            "{results:?}"
        );
        assert_eq!(db.withdrawals_of_identity(IDENTITY).unwrap().len(), 1);
    }
}

#[test]
fn the_unique_index_covers_the_open_states_only() {
    let db = AppDb::open_in_memory().unwrap();
    db.insert_withdrawal_durable(&signed("a", 3), 0).unwrap();
    for open in [Signed, Sent, Refused, CouldntConfirm] {
        if open != Signed {
            let from = state(&db, "a");
            assert!(
                db.set_withdrawal_state_durable("a", &[from], open, None)
                    .unwrap()
            );
        }
        assert!(matches!(
            db.insert_withdrawal_durable(&signed("b", 4), 0),
            Err(AppDbError::WithdrawalPending(_))
        ));
        // The index alone, without this crate's insert.
        let raw = db.conn().execute(
            "INSERT INTO mn_withdrawals (id, transition_hash, created_by, pro_tx_hash,
                 identity_id, key_kind, amount, core_fee_per_byte, pooling, expected_duffs,
                 nonce, signed_bytes, expected_script, signed_at, state)
             SELECT 'raw', transition_hash, created_by, pro_tx_hash, identity_id, key_kind,
                 amount, core_fee_per_byte, pooling, expected_duffs, 9, signed_bytes,
                 expected_script, signed_at, 'signed' FROM mn_withdrawals WHERE id = 'a'",
            [],
        );
        assert!(raw.unwrap_err().to_string().contains("UNIQUE"), "{open:?}");
    }
    // Another identity is not held.
    let mut other = signed("c", 1);
    other.identity_id = "ident-2".into();
    db.insert_withdrawal_durable(&other, 0).unwrap();
    // Marked: closed for the open set (P9).
    db.mark_withdrawal_durable("a", WithdrawalMark::Received, 9)
        .unwrap();
    db.insert_withdrawal_durable(&signed("b", 3), 0).unwrap();
    assert_eq!(
        db.open_withdrawal(IDENTITY).unwrap().unwrap().signed.id,
        "b"
    );
}

/// MG-13 P15: Try again moves the refused record out of the open set and
/// inserts the sibling in one transaction, where a plain insert hits the
/// index.
#[test]
fn supersede_and_insert_works_where_a_plain_insert_hits_the_index() {
    let db = AppDb::open_in_memory().unwrap();
    db.insert_withdrawal_durable(&signed("a", 3), 12).unwrap();
    db.set_withdrawal_state_durable("a", &[Signed], Refused, Some("merits"))
        .unwrap();
    assert!(matches!(
        db.insert_withdrawal_durable(&signed("b", 3), 0),
        Err(AppDbError::WithdrawalPending(_))
    ));
    let mut b = signed("b", 3);
    b.amount = 2_000_000;
    assert!(
        db.supersede_and_insert_withdrawal_durable("a", &b, 0)
            .unwrap()
    );
    assert_eq!(state(&db, "a"), Superseded);
    let read = db.withdrawal("b").unwrap().unwrap();
    assert_eq!(
        (read.state, read.floor_height),
        (Signed, 12),
        "the group's floor"
    );
    assert_eq!(
        db.open_withdrawal(IDENTITY).unwrap().unwrap().signed.id,
        "b"
    );
    assert_eq!(db.withdrawal_group(IDENTITY, 3).unwrap().len(), 2);

    // Only from `refused`, and only into its own group.
    assert!(
        !db.supersede_and_insert_withdrawal_durable("b", &signed("c", 3), 0)
            .unwrap()
    );
    assert!(
        !db.supersede_and_insert_withdrawal_durable("gone", &signed("c", 3), 0)
            .unwrap()
    );
    db.set_withdrawal_state_durable("b", &[Signed], Refused, None)
        .unwrap();
    for wrong in [
        signed("c", 4),
        SignedWithdrawal {
            identity_id: "ident-2".into(),
            ..signed("c", 3)
        },
        SignedWithdrawal {
            pro_tx_hash: PRO_TX2.into(),
            ..signed("c", 3)
        },
    ] {
        assert!(matches!(
            db.supersede_and_insert_withdrawal_durable("b", &wrong, 0),
            Err(AppDbError::Invalid(_))
        ));
    }
    assert!(db.withdrawal("c").unwrap().is_none());
    assert_eq!(state(&db, "b"), Refused);
}

/// MG-13 §3: the supersede and the insert are one transaction. A failure
/// between them (the fault point), or of the insert, leaves the refused
/// record `refused` and no sibling.
#[test]
fn supersede_and_insert_is_atomic() {
    let (_dir, path, db) = file_db();
    db.insert_withdrawal_durable(&signed("a", 3), 0).unwrap();
    db.set_withdrawal_state_durable("a", &[Signed], Refused, None)
        .unwrap();
    fault::arm(&path, fault::MN_WITHDRAW_SUPERSEDE, Stage::Before);
    assert!(matches!(
        db.supersede_and_insert_withdrawal_durable("a", &signed("b", 3), 0),
        Err(AppDbError::Fault(name)) if name == "mn_withdraw.before_supersede"
    ));
    assert_eq!(state(&db, "a"), Refused);
    assert!(db.withdrawal("b").unwrap().is_none());

    db.conn()
        .execute_batch(
            "CREATE TEMP TRIGGER no_sibling BEFORE INSERT ON mn_withdrawals
             BEGIN SELECT RAISE(ABORT, 'insert refused'); END;",
        )
        .unwrap();
    assert!(
        db.supersede_and_insert_withdrawal_durable("a", &signed("b", 3), 0)
            .is_err()
    );
    assert_eq!(state(&db, "a"), Refused);
    db.conn().execute_batch("DROP TRIGGER no_sibling").unwrap();

    // A barrier that appeared: refused, nothing written.
    db.insert_masternode_op_with_barrier_durable(W, &update_op("u", true), &[], 1, Some(&[9]))
        .unwrap();
    assert!(matches!(
        db.supersede_and_insert_withdrawal_durable("a", &signed("b", 3), 0),
        Err(AppDbError::PayoutChangePending(p)) if p == PRO_TX
    ));
    assert_eq!(state(&db, "a"), Refused);
    assert!(db.withdrawal("b").unwrap().is_none());
}

/// MG-13 P5: a group's floor only rises, every sibling carries it, and
/// another group's is its own.
#[test]
fn the_floor_only_rises_and_covers_every_sibling() {
    let db = group_with_x_in(Signed);
    let mut other = signed("n", 4);
    other.identity_id = "ident-2".into();
    db.insert_withdrawal_durable(&other, 3).unwrap();
    let floors = |db: &AppDb| {
        (db.withdrawal_group(IDENTITY, 3).unwrap().iter())
            .map(|r| r.floor_height)
            .collect::<Vec<_>>()
    };
    assert_eq!(floors(&db), [0, 0]);
    assert_eq!(
        db.raise_withdrawal_floor_durable(IDENTITY, 3, 10).unwrap(),
        Some(10)
    );
    assert_eq!(floors(&db), [10, 10]);
    assert_eq!(
        db.raise_withdrawal_floor_durable(IDENTITY, 3, 5).unwrap(),
        Some(10),
        "never lowered"
    );
    assert_eq!(floors(&db), [10, 10]);
    assert_eq!(db.withdrawal("n").unwrap().unwrap().floor_height, 3);
    assert_eq!(
        db.raise_withdrawal_floor_durable(IDENTITY, 9, 5).unwrap(),
        None
    );
    // A sibling inserted later starts at the group's floor.
    db.set_withdrawal_state_durable("x", &[Signed], Refused, None)
        .unwrap();
    db.supersede_and_insert_withdrawal_durable("x", &signed("y", 3), 1)
        .unwrap();
    assert_eq!(floors(&db), [10, 10, 10]);
    // A floor written unevenly (by no write here) is evened up to its max.
    db.conn()
        .execute(
            "UPDATE mn_withdrawals SET floor_height = 20 WHERE id = 'o'",
            [],
        )
        .unwrap();
    assert_eq!(
        db.raise_withdrawal_floor_durable(IDENTITY, 3, 15).unwrap(),
        Some(20)
    );
    assert_eq!(floors(&db), [20, 20, 20]);
    assert!(
        db.conn()
            .execute(
                "UPDATE mn_withdrawals SET floor_height = 1 WHERE id = 'o'",
                []
            )
            .is_err()
    );
}

#[test]
fn a_note_keeps_what_it_does_not_set() {
    let db = AppDb::open_in_memory().unwrap();
    assert!(
        !db.note_withdrawal_durable("a", &WithdrawalNote::default())
            .unwrap()
    );
    db.insert_withdrawal_durable(&signed("a", 3), 0).unwrap();
    let note = WithdrawalNote {
        document_status: Some(3),
        last_checked: Some(77),
        late_proof_link: Some("b".into()),
        state_reason: Some("why".into()),
    };
    assert!(db.note_withdrawal_durable("a", &note).unwrap());
    let partial = WithdrawalNote {
        last_checked: Some(78),
        ..WithdrawalNote::default()
    };
    assert!(db.note_withdrawal_durable("a", &partial).unwrap());
    let a = db.withdrawal("a").unwrap().unwrap();
    assert_eq!(
        (
            a.document_status,
            a.last_checked,
            a.late_proof_link.as_deref(),
            a.state_reason.as_deref(),
            a.state
        ),
        (Some(3), Some(78), Some("b"), Some("why"), Signed)
    );
}

/// An `update_registrar` operation changing the payout of `PRO_TX`.
fn update_op(id: &str, signed: bool) -> MasternodeOpRow {
    MasternodeOpRow {
        id: id.into(),
        kind: "update_registrar".into(),
        terms: "{}".into(),
        request: "{}".into(),
        unsigned_tx: (!signed).then(|| vec![1]),
        sign_address: None,
        sign_message: None,
        fee_inputs: "[]".into(),
        signed_tx: signed.then(|| vec![2]),
        txid: signed.then(|| TXID.into()),
        pro_tx_hash: Some(PRO_TX.into()),
        locked_collateral: false,
        fee: 7,
        created_at: 1,
        signed_at: signed.then_some(2),
        sent_at: None,
        last_error: None,
    }
}

fn barrier_ops(db: &AppDb) -> Vec<(String, String)> {
    (db.payout_barriers().unwrap().into_iter())
        .map(|b| (b.barrier.created_by, b.barrier.op_id))
        .collect()
}

/// MG-13 P16: both op writers write the barrier with the signed bytes, in
/// one transaction; none without a payout change; a record insert is refused
/// while one exists.
#[test]
fn a_barrier_is_written_with_its_operations_signed_bytes() {
    let db = AppDb::open_in_memory().unwrap();
    db.insert_masternode_op_with_barrier_durable(W, &update_op("a", true), &[], 1, Some(&[9]))
        .unwrap();
    let barriers = db.payout_barriers_of(PRO_TX).unwrap();
    assert_eq!(barriers.len(), 1);
    assert_eq!(
        barriers[0].barrier,
        NewPayoutBarrier {
            pro_tx_hash: PRO_TX.into(),
            op_id: "a".into(),
            created_by: W.into(),
            txid: TXID.into(),
            payout_script: vec![9],
        }
    );
    assert_eq!(barriers[0].created_at, 1);
    assert!(db.payout_barriers_of(PRO_TX2).unwrap().is_empty());

    // The plain writers write none.
    db.insert_masternode_op_durable(W, &update_op("b", true), &[], 1)
        .unwrap();
    db.insert_masternode_op_durable(W, &update_op("c", false), &[], 1)
        .unwrap();
    db.sign_masternode_op_durable(W, "c", &[4], TXID, PRO_TX, 3)
        .unwrap();
    assert_eq!(barrier_ops(&db), [(W.into(), "a".into())]);

    // An External row: the barrier with its signature, once.
    db.insert_masternode_op_with_barrier_durable(W, &update_op("e", false), &[], 1, None)
        .unwrap();
    assert!(
        db.sign_masternode_op_with_barrier_durable(W, "e", &[4], TXID, PRO_TX, 3, Some(&[8]))
            .unwrap()
    );
    assert!(
        !db.sign_masternode_op_with_barrier_durable(W, "e", &[5], TXID, PRO_TX, 4, Some(&[7]))
            .unwrap()
    );
    assert!(
        !db.sign_masternode_op_with_barrier_durable(W, "gone", &[5], TXID, PRO_TX, 4, Some(&[7]))
            .unwrap()
    );
    let e = db.payout_barriers_of(PRO_TX).unwrap().pop().unwrap();
    assert_eq!(
        (
            e.barrier.op_id.as_str(),
            e.barrier.payout_script,
            e.created_at
        ),
        ("e", vec![8], 3)
    );
    assert_eq!(db.payout_barriers().unwrap().len(), 2);

    // A barrier needs signed bytes.
    assert!(matches!(
        db.insert_masternode_op_with_barrier_durable(W, &update_op("u", false), &[], 1, Some(&[9])),
        Err(AppDbError::Invalid(_))
    ));
    assert!(db.masternode_op(W, "u").unwrap().is_none());

    // The record insert is refused while a barrier of the masternode exists.
    assert!(matches!(
        db.insert_withdrawal_durable(&signed("w", 3), 0),
        Err(AppDbError::PayoutChangePending(p)) if p == PRO_TX
    ));
    let mut other = signed("w", 3);
    other.pro_tx_hash = PRO_TX2.into();
    other.identity_id = "ident-2".into();
    db.insert_withdrawal_durable(&other, 0).unwrap();
    for barrier in db.payout_barriers_of(PRO_TX).unwrap() {
        assert!(db.clear_payout_barrier_durable(barrier.id).unwrap());
        assert!(!db.clear_payout_barrier_durable(barrier.id).unwrap());
    }
    db.insert_withdrawal_durable(&signed("w2", 3), 0).unwrap();
}

/// The row and its barrier are one write: a failure writing either leaves
/// neither; a removed wallet's row (skipped, DEC-210) gets no barrier.
#[test]
fn an_operation_and_its_barrier_are_written_together() {
    let db = AppDb::open_in_memory().unwrap();
    db.conn()
        .execute_batch(
            "CREATE TEMP TRIGGER no_barrier BEFORE INSERT ON mn_payout_barriers
             BEGIN SELECT RAISE(ABORT, 'barrier refused'); END;",
        )
        .unwrap();
    assert!(
        db.insert_masternode_op_with_barrier_durable(W, &update_op("a", true), &[], 1, Some(&[9]))
            .is_err()
    );
    assert!(
        db.masternode_op(W, "a").unwrap().is_none(),
        "no row without its barrier"
    );
    db.insert_masternode_op_durable(W, &update_op("e", false), &[], 1)
        .unwrap();
    assert!(
        db.sign_masternode_op_with_barrier_durable(W, "e", &[4], TXID, PRO_TX, 3, Some(&[8]))
            .is_err()
    );
    assert_eq!(db.masternode_op(W, "e").unwrap().unwrap().signed_tx, None);
    db.conn()
        .execute_batch(
            "DROP TRIGGER no_barrier;
             CREATE TEMP TRIGGER no_row BEFORE INSERT ON masternode_ops
             BEGIN SELECT RAISE(ABORT, 'row refused'); END;",
        )
        .unwrap();
    assert!(
        db.insert_masternode_op_with_barrier_durable(W, &update_op("a", true), &[], 1, Some(&[9]))
            .is_err()
    );
    assert!(
        db.payout_barriers().unwrap().is_empty(),
        "no barrier without its row"
    );
    db.conn().execute_batch("DROP TRIGGER no_row").unwrap();

    db.delete_wallet(W2).unwrap();
    db.insert_masternode_op_with_barrier_durable(W2, &update_op("r", true), &[], 1, Some(&[9]))
        .unwrap();
    assert!(db.masternode_op(W2, "r").unwrap().is_none());
    assert!(db.payout_barriers().unwrap().is_empty());
}

/// MG-13 P16 (r4 m1): the discard deletes the unsent operation and its
/// barrier in one transaction; a discard refused by `sent_at` keeps both,
/// and a failure deleting the barrier keeps the operation.
#[test]
fn the_discard_deletes_the_operation_and_its_barrier_together() {
    let db = AppDb::open_in_memory().unwrap();
    for id in ["a", "b", "c"] {
        db.insert_masternode_op_with_barrier_durable(W, &update_op(id, true), &[], 1, Some(&[9]))
            .unwrap();
    }
    // Another wallet's operation of the same id keeps its barrier.
    db.insert_masternode_op_with_barrier_durable(W2, &update_op("a", true), &[], 1, Some(&[9]))
        .unwrap();
    assert!(db.discard_unsent_masternode_op_durable(W, "a").unwrap());
    assert!(db.masternode_op(W, "a").unwrap().is_none());
    assert_eq!(
        barrier_ops(&db),
        [
            (W.into(), "b".into()),
            (W.into(), "c".into()),
            (W2.into(), "a".into())
        ]
    );

    // Sent: refused, both kept.
    db.begin_masternode_op_attempt(W, "b", 5).unwrap();
    assert!(!db.discard_unsent_masternode_op_durable(W, "b").unwrap());
    assert!(db.masternode_op(W, "b").unwrap().is_some());
    assert!(barrier_ops(&db).contains(&(W.into(), "b".into())));
    assert!(!db.discard_unsent_masternode_op_durable(W, "gone").unwrap());

    db.conn()
        .execute_batch(
            "CREATE TEMP TRIGGER keep_barrier BEFORE DELETE ON mn_payout_barriers
             BEGIN SELECT RAISE(ABORT, 'delete refused'); END;",
        )
        .unwrap();
    assert!(db.discard_unsent_masternode_op_durable(W, "c").is_err());
    assert!(
        db.masternode_op(W, "c").unwrap().is_some(),
        "no row deleted alone"
    );
    db.conn()
        .execute_batch("DROP TRIGGER keep_barrier")
        .unwrap();
    assert!(db.discard_unsent_masternode_op_durable(W, "c").unwrap());
    assert!(!barrier_ops(&db).contains(&(W.into(), "c".into())));
}

/// MG-13 P16: the upgrade's backfill writes each barrier once, keeps one
/// that exists, and sets its marker in the same transaction; a second run
/// writes nothing.
#[test]
fn the_backfill_writes_once_with_its_marker() {
    let db = AppDb::open_in_memory().unwrap();
    db.insert_masternode_op_with_barrier_durable(W, &update_op("a", true), &[], 1, Some(&[9]))
        .unwrap();
    let barrier = |op: &str, script: u8| NewPayoutBarrier {
        pro_tx_hash: PRO_TX.into(),
        op_id: op.into(),
        created_by: W.into(),
        txid: TXID.into(),
        payout_script: vec![script],
    };
    assert!(!db.payout_barriers_backfilled().unwrap());
    // A failure writes neither the barriers nor the marker.
    let bad = NewPayoutBarrier {
        txid: "short".into(),
        ..barrier("x", 1)
    };
    assert!(
        db.backfill_payout_barriers_durable(&[barrier("b", 1), bad], 5)
            .is_err()
    );
    assert!(!db.payout_barriers_backfilled().unwrap());
    assert_eq!(db.payout_barriers().unwrap().len(), 1);

    assert_eq!(
        db.backfill_payout_barriers_durable(&[barrier("a", 1), barrier("b", 2)], 5)
            .unwrap(),
        Some(1)
    );
    assert!(db.payout_barriers_backfilled().unwrap());
    let rows = db.payout_barriers().unwrap();
    assert_eq!(
        (rows.iter())
            .map(|r| (r.barrier.op_id.as_str(), r.barrier.payout_script[0]))
            .collect::<Vec<_>>(),
        [("a", 9), ("b", 2)],
        "the existing barrier kept"
    );
    assert_eq!(
        db.backfill_payout_barriers_durable(&[barrier("c", 3)], 6)
            .unwrap(),
        None
    );
    assert_eq!(db.payout_barriers().unwrap().len(), 2);
}

/// MG-13 P5, P16, §3: a wallet removal keeps both tables' rows.
#[test]
fn a_wallet_removal_keeps_records_and_barriers() {
    assert!(!crate::WALLET_TABLES.contains(&"mn_withdrawals"));
    assert!(!crate::WALLET_TABLES.contains(&"mn_payout_barriers"));
    let db = AppDb::open_in_memory().unwrap();
    db.insert_masternode_op_with_barrier_durable(W, &update_op("a", true), &[], 1, Some(&[9]))
        .unwrap();
    let mut row = signed("w", 3);
    row.pro_tx_hash = PRO_TX2.into();
    db.insert_withdrawal_durable(&row, 0).unwrap();
    db.delete_wallet(W).unwrap();
    assert!(db.masternode_op(W, "a").unwrap().is_none());
    assert_eq!(barrier_ops(&db), [(W.into(), "a".into())]);
    assert_eq!(db.withdrawal("w").unwrap().unwrap().signed.created_by, W);
    // And a backup carries neither.
    let exported = db.export_wallet_rows(W).unwrap();
    assert!(
        exported
            .iter()
            .all(|t| t.table != "mn_withdrawals" && t.table != "mn_payout_barriers")
    );
}

#[test]
fn a_fault_point_has_a_before_and_an_after_name() {
    assert_eq!(
        fault::name(fault::MN_WITHDRAW_INSERT, Stage::After),
        "mn_withdraw.after_insert"
    );
    assert_eq!(
        fault::name(fault::MN_BARRIER_DISCARD, Stage::Before),
        "mn_barrier.before_discard"
    );
    let mut names: Vec<_> = (fault::POINTS.iter())
        .flat_map(|p| [Stage::Before, Stage::After].map(|s| fault::name(p, s)))
        .collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), 2 * fault::POINTS.len());
}

/// One fault point's write: its setup, the write, and whether the write is
/// on disk.
struct Case {
    point: &'static str,
    setup: fn(&AppDb),
    write: fn(&AppDb) -> crate::Result<()>,
    written: fn(&AppDb) -> bool,
}

fn refused_a(db: &AppDb) {
    db.insert_withdrawal_durable(&signed("a", 3), 0).unwrap();
    db.set_withdrawal_state_durable("a", &[Signed], Refused, None)
        .unwrap();
}

fn couldnt_confirm_a(db: &AppDb) {
    db.insert_withdrawal_durable(&signed("a", 3), 0).unwrap();
    db.set_withdrawal_state_durable("a", &[Signed], CouldntConfirm, None)
        .unwrap();
}

fn signed_a(db: &AppDb) {
    db.insert_withdrawal_durable(&signed("a", 3), 0).unwrap();
}

fn signed_op(db: &AppDb) {
    db.insert_masternode_op_with_barrier_durable(W, &update_op("op", true), &[], 1, Some(&[9]))
        .unwrap();
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            point: fault::MN_WITHDRAW_INSERT,
            setup: |_| {},
            write: |db| db.insert_withdrawal_durable(&signed("a", 3), 0),
            written: |db| db.withdrawal("a").unwrap().is_some(),
        },
        Case {
            point: fault::MN_WITHDRAW_SENT_AT,
            setup: signed_a,
            write: |db| db.begin_withdrawal_send_durable("a", 5).map(drop),
            written: |db| db.withdrawal("a").unwrap().unwrap().sent_at.is_some(),
        },
        Case {
            point: fault::MN_WITHDRAW_STATE,
            setup: signed_a,
            write: |db| {
                db.set_withdrawal_state_durable("a", &[Signed], Sent, None)
                    .map(drop)
            },
            written: |db| state(db, "a") == Sent,
        },
        Case {
            point: fault::MN_WITHDRAW_FINALIZE,
            setup: signed_a,
            write: |db| {
                db.finalize_withdrawal_group_durable("a", &[Signed], &proof())
                    .map(drop)
            },
            written: |db| state(db, "a") == Done,
        },
        Case {
            point: fault::MN_WITHDRAW_NOT_EXECUTED,
            setup: signed_a,
            write: |db| {
                db.not_executed_withdrawal_group_durable(IDENTITY, 3, &[("a", Signed)], &evidence())
                    .map(drop)
            },
            written: |db| state(db, "a") == NotExecuted,
        },
        Case {
            point: fault::MN_WITHDRAW_SUPERSEDE,
            setup: refused_a,
            write: |db| {
                db.supersede_and_insert_withdrawal_durable("a", &signed("b", 3), 0)
                    .map(drop)
            },
            written: |db| {
                let b = db.withdrawal("b").unwrap().is_some();
                assert_eq!(b, state(db, "a") == Superseded, "one transaction");
                b
            },
        },
        Case {
            point: fault::MN_WITHDRAW_MARK,
            setup: couldnt_confirm_a,
            write: |db| {
                db.mark_withdrawal_durable("a", WithdrawalMark::Received, 9)
                    .map(drop)
            },
            written: |db| state(db, "a") == MarkedReceived,
        },
        Case {
            point: fault::MN_WITHDRAW_FLOOR,
            setup: signed_a,
            write: |db| db.raise_withdrawal_floor_durable(IDENTITY, 3, 30).map(drop),
            written: |db| db.withdrawal("a").unwrap().unwrap().floor_height == 30,
        },
        Case {
            point: fault::MN_WITHDRAW_NOTE,
            setup: signed_a,
            write: |db| {
                let note = WithdrawalNote {
                    last_checked: Some(4),
                    ..WithdrawalNote::default()
                };
                db.note_withdrawal_durable("a", &note).map(drop)
            },
            written: |db| db.withdrawal("a").unwrap().unwrap().last_checked == Some(4),
        },
        Case {
            point: fault::MN_BARRIER_WRITE,
            setup: |_| {},
            write: |db| {
                db.insert_masternode_op_with_barrier_durable(
                    W,
                    &update_op("op", true),
                    &[],
                    1,
                    Some(&[9]),
                )
            },
            written: |db| {
                let op = db.masternode_op(W, "op").unwrap().is_some();
                assert_eq!(op, !db.payout_barriers().unwrap().is_empty(), "one write");
                op
            },
        },
        Case {
            point: fault::MN_BARRIER_WRITE,
            setup: |db| {
                db.insert_masternode_op_durable(W, &update_op("op", false), &[], 1)
                    .unwrap()
            },
            write: |db| {
                db.sign_masternode_op_with_barrier_durable(
                    W,
                    "op",
                    &[4],
                    TXID,
                    PRO_TX,
                    3,
                    Some(&[8]),
                )
                .map(drop)
            },
            written: |db| {
                let signed = db.masternode_op(W, "op").unwrap().unwrap().signed_tx;
                assert_eq!(
                    signed.is_some(),
                    !db.payout_barriers().unwrap().is_empty(),
                    "one write"
                );
                signed.is_some()
            },
        },
        Case {
            point: fault::MN_BARRIER_CLEAR,
            setup: signed_op,
            write: |db| {
                let id = db.payout_barriers().unwrap()[0].id;
                db.clear_payout_barrier_durable(id).map(drop)
            },
            written: |db| db.payout_barriers().unwrap().is_empty(),
        },
        Case {
            point: fault::MN_BARRIER_DISCARD,
            setup: signed_op,
            write: |db| db.discard_unsent_masternode_op_durable(W, "op").map(drop),
            written: |db| {
                let gone = db.masternode_op(W, "op").unwrap().is_none();
                assert_eq!(gone, db.payout_barriers().unwrap().is_empty(), "together");
                gone
            },
        },
        Case {
            point: fault::MN_BARRIER_BACKFILL,
            setup: |_| {},
            write: |db| {
                let barrier = NewPayoutBarrier {
                    pro_tx_hash: PRO_TX.into(),
                    op_id: "op".into(),
                    created_by: W.into(),
                    txid: TXID.into(),
                    payout_script: vec![1],
                };
                db.backfill_payout_barriers_durable(&[barrier], 1).map(drop)
            },
            written: |db| {
                let marked = db.payout_barriers_backfilled().unwrap();
                assert_eq!(
                    marked,
                    !db.payout_barriers().unwrap().is_empty(),
                    "together"
                );
                marked
            },
        },
    ]
}

/// MG-13 §3 fault sweep: each point, armed before, fails its write with
/// nothing written; armed after, fails it with the write on disk (seen
/// again after a reopen); either fires once.
#[test]
fn every_fault_point_does_what_it_says() {
    let cases = cases();
    let covered: std::collections::HashSet<_> = cases.iter().map(|c| c.point).collect();
    assert_eq!(covered.len(), fault::POINTS.len(), "a case per point");
    for case in &cases {
        for stage in [Stage::Before, Stage::After] {
            let (_dir, path, db) = file_db();
            (case.setup)(&db);
            fault::arm(&path, case.point, stage);
            let name = fault::name(case.point, stage);
            match (case.write)(&db) {
                Err(AppDbError::Fault(n)) if n == name => {}
                other => panic!("{name}: {other:?}"),
            }
            drop(db);
            let db = AppDb::open(&path).unwrap();
            assert_eq!((case.written)(&db), stage == Stage::After, "{name}");
            if stage == Stage::Before {
                // Disarmed: the write now goes through.
                (case.write)(&db).unwrap_or_else(|e| panic!("{name} again: {e}"));
                assert!((case.written)(&db), "{name} again");
            }
        }
    }
}

/// A point armed for one database leaves another alone, and `disarm`
/// clears it.
#[test]
fn a_fault_point_is_armed_per_database() {
    let (_dir, path, db) = file_db();
    let (_dir2, _path2, db2) = file_db();
    fault::arm(&path, fault::MN_WITHDRAW_INSERT, Stage::Before);
    db2.insert_withdrawal_durable(&signed("a", 3), 0).unwrap();
    fault::disarm(&path);
    db.insert_withdrawal_durable(&signed("a", 3), 0).unwrap();
    // The engine's own points.
    fault::arm(&path, "mn_withdraw.broadcast", Stage::Before);
    assert!(matches!(
        db.fault_point("mn_withdraw.broadcast", Stage::Before),
        Err(AppDbError::Fault(n)) if n == "mn_withdraw.before_broadcast"
    ));
    db.fault_point("mn_withdraw.broadcast", Stage::Before)
        .unwrap();
    assert!(fault::hit(&path, "mn_withdraw.broadcast", Stage::After).is_ok());
}

/// Run by [`dw_fault_aborts_after_the_insert`] as its child process (a test
/// that does nothing otherwise): inserts a record into the database
/// `DW_APPDB_FAULT_CHILD` names, with `DW_FAULT` set.
#[test]
fn child_inserting_with_dw_fault() {
    let Some(path) = std::env::var_os("DW_APPDB_FAULT_CHILD") else {
        return;
    };
    let db = AppDb::open(Path::new(&path)).unwrap();
    let _ = db.insert_withdrawal_durable(&signed("a", 3), 0);
    println!("returned");
}

/// MG-13 T2 (4): `DW_FAULT=mn_withdraw.after_insert` aborts the process
/// right after the record's insert committed; the record is on disk.
#[test]
fn dw_fault_aborts_after_the_insert() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(crate::APP_DB_FILE);
    drop(AppDb::open(&path).unwrap());
    let starting = crate::CHILD_STARTS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "mn_withdrawals::tests::child_inserting_with_dw_fault",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("DW_APPDB_FAULT_CHILD", &path)
        .env("DW_FAULT", "mn_withdraw.after_insert")
        .output()
        .unwrap();
    drop(starting);
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("returned"), "{stdout}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("DW_FAULT=mn_withdraw.after_insert"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let db = AppDb::open(&path).unwrap();
    assert_eq!(state(&db, "a"), Signed);
}
