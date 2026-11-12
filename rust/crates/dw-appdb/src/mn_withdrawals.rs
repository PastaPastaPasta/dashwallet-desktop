//! The single writer of `mn_withdrawals` (MG-13 P5, P6, P15): evonode
//! Platform credit withdrawal records, app-wide and keyed by identity.
//!
//! A record is written in one synced transaction before any network step
//! and never deleted. Every state change is a compare-and-set against what
//! the caller read, checked against P6's table here
//! ([`WithdrawalState::may_become`]) and again by the schema's trigger. The
//! partial unique index `mn_withdrawals_open` keeps one open record per
//! identity across processes; an insert it refuses is
//! [`AppDbError::WithdrawalPending`]. An insert also refuses while a
//! payout-change barrier of the masternode exists
//! ([`AppDbError::PayoutChangePending`], P4 (b), r4 n1). Writes a crash must
//! not lose are synced ([`AppDb::durably`]) and carry a fault point
//! ([`crate::fault_points`]).

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::fault_points::{
    MN_WITHDRAW_FINALIZE, MN_WITHDRAW_FLOOR, MN_WITHDRAW_INSERT, MN_WITHDRAW_MARK,
    MN_WITHDRAW_NOT_EXECUTED, MN_WITHDRAW_NOTE, MN_WITHDRAW_SENT_AT, MN_WITHDRAW_STATE,
    MN_WITHDRAW_SUPERSEDE,
};
use crate::{AppDb, AppDbError, Result, fault_before, sql_u64, write_transaction};

/// The key that signed (MG-13 P2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WithdrawalKeyKind {
    /// The destination is Drive's at execution; the record keeps the one the
    /// quote saw (`expected_script`).
    Owner,
    /// The destination is the signed `output_script` (`script`).
    Payout,
}

impl WithdrawalKeyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Payout => "payout",
        }
    }

    fn parse(s: &str) -> rusqlite::Result<Self> {
        match s {
            "owner" => Ok(Self::Owner),
            "payout" => Ok(Self::Payout),
            other => Err(corrupt("key_kind", other)),
        }
    }
}

/// A record's state (MG-13 P6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WithdrawalState {
    Signed,
    Sent,
    Refused,
    Superseded,
    CouldntConfirm,
    MarkedReceived,
    MarkedNotReceived,
    Done,
    SiblingPaid,
    NotExecuted,
}

impl WithdrawalState {
    pub const ALL: [Self; 10] = [
        Self::Signed,
        Self::Sent,
        Self::Refused,
        Self::Superseded,
        Self::CouldntConfirm,
        Self::MarkedReceived,
        Self::MarkedNotReceived,
        Self::Done,
        Self::SiblingPaid,
        Self::NotExecuted,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Signed => "signed",
            Self::Sent => "sent",
            Self::Refused => "refused",
            Self::Superseded => "superseded",
            Self::CouldntConfirm => "couldnt_confirm",
            Self::MarkedReceived => "marked_received",
            Self::MarkedNotReceived => "marked_not_received",
            Self::Done => "done",
            Self::SiblingPaid => "sibling_paid",
            Self::NotExecuted => "not_executed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == s)
    }

    /// Open: Withdraw is blocked, and the unique index covers it (P6).
    pub fn is_open(self) -> bool {
        matches!(
            self,
            Self::Signed | Self::Sent | Self::Refused | Self::CouldntConfirm
        )
    }

    /// Final: nothing leaves it (P6).
    pub fn is_final(self) -> bool {
        matches!(self, Self::Done | Self::SiblingPaid | Self::NotExecuted)
    }

    /// P6's table: whether a record in `self` may move to `to`. No final
    /// state is left, and no record that is not open enters the open set
    /// (the schema's `mn_withdrawals_transitions` trigger is the same table).
    pub fn may_become(self, to: Self) -> bool {
        use WithdrawalState::*;
        match (self, to) {
            (Signed, Sent) => true,
            (Signed | Sent, Refused) => true,
            (Refused, Superseded) => true,
            (Signed | Sent | Refused, CouldntConfirm) => true,
            (CouldntConfirm, MarkedReceived | MarkedNotReceived) => true,
            (from, Done | SiblingPaid | NotExecuted) => !from.is_final(),
            _ => false,
        }
    }
}

/// The user's mark of a record that couldn't be confirmed (MG-13 P9, D47).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WithdrawalMark {
    Received,
    NotReceived,
}

impl WithdrawalMark {
    fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::NotReceived => "not_received",
        }
    }

    fn state(self) -> WithdrawalState {
        match self {
            Self::Received => WithdrawalState::MarkedReceived,
            Self::NotReceived => WithdrawalState::MarkedNotReceived,
        }
    }

    fn parse(s: &str) -> rusqlite::Result<Self> {
        match s {
            "received" => Ok(Self::Received),
            "not_received" => Ok(Self::NotReceived),
            other => Err(corrupt("user_mark", other)),
        }
    }
}

/// What was signed: written once, never changed (MG-13 P5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedWithdrawal {
    /// Engine-chosen.
    pub id: String,
    /// 64 lower-case hex.
    pub transition_hash: String,
    /// The wallet that wrote it: a label, not a key a wallet removal uses.
    pub created_by: String,
    /// 64 lower-case hex, as displayed.
    pub pro_tx_hash: String,
    /// The evonode's identity, in the engine's encoding.
    pub identity_id: String,
    pub key_kind: WithdrawalKeyKind,
    /// Credits.
    pub amount: u64,
    pub core_fee_per_byte: u32,
    /// dpp `Pooling` as its integer.
    pub pooling: u8,
    pub expected_duffs: u64,
    pub nonce: u64,
    pub signed_bytes: Vec<u8>,
    /// The payout key's signed `output_script`; `None` for the owner key.
    pub script: Option<Vec<u8>>,
    /// The owner key's destination as the quote saw it; `None` for the
    /// payout key.
    pub expected_script: Option<Vec<u8>>,
    pub signed_at: u64,
}

/// The proved withdrawal document behind `done` (MG-13 P7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalProof {
    pub document_id: String,
    /// The proof's Platform height and block time.
    pub height: u64,
    pub block_time: u64,
    /// The document's `outputScript`.
    pub paid_script: Vec<u8>,
    /// The document's `status` (Drive's integer).
    pub document_status: u8,
}

/// A `done` record's proved document, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvedDocument {
    pub document_id: String,
    pub height: u64,
    pub block_time: Option<u64>,
}

/// P8's heights behind a group's `not_executed`: (i) the nonce read at H,
/// (ii) the identity read for the candidates, (iv) the document reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotExecutedEvidence {
    pub nonce_height: u64,
    pub identity_height: u64,
    pub document_height: u64,
    /// Stored as every record's `state_reason`.
    pub reason: Option<String>,
}

/// What a recheck notes without changing the state; `None` keeps a value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WithdrawalNote {
    pub document_status: Option<u8>,
    pub last_checked: Option<u64>,
    /// The later record a late proof points at (P14).
    pub late_proof_link: Option<String>,
    /// The current refusal or undecidable read of an open record.
    pub state_reason: Option<String>,
}

/// One `mn_withdrawals` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalRow {
    pub signed: SignedWithdrawal,
    pub state: WithdrawalState,
    pub state_reason: Option<String>,
    pub sent_at: Option<u64>,
    pub paid_script: Option<Vec<u8>>,
    pub document_status: Option<u8>,
    pub proof: Option<ProvedDocument>,
    pub evidence: Option<NotExecutedEvidence>,
    pub floor_height: u64,
    pub last_checked: Option<u64>,
    /// The user's mark and its time, apart from the proof (P9).
    pub mark: Option<(WithdrawalMark, u64)>,
    pub late_proof_link: Option<String>,
}

const COLUMNS: &str = "id, transition_hash, created_by, pro_tx_hash, identity_id, key_kind, \
     amount, core_fee_per_byte, pooling, expected_duffs, nonce, signed_bytes, script, \
     expected_script, signed_at, state, state_reason, sent_at, paid_script, document_status, \
     proof_document_id, proof_height, proof_block_time, evidence_nonce_height, \
     evidence_identity_height, evidence_document_height, floor_height, last_checked, user_mark, \
     marked_at, late_proof_link";

const FINAL_STATES: &str = "('done', 'sibling_paid', 'not_executed')";

fn corrupt(column: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("mn_withdrawals.{column} {value:?}").into(),
    )
}

fn get_u64(r: &Row<'_>, i: usize) -> rusqlite::Result<u64> {
    Ok(u64::try_from(r.get::<_, i64>(i)?).unwrap_or(0))
}

fn get_opt_u64(r: &Row<'_>, i: usize) -> rusqlite::Result<Option<u64>> {
    Ok(r.get::<_, Option<i64>>(i)?
        .and_then(|v| u64::try_from(v).ok()))
}

fn read_row(r: &Row<'_>) -> rusqlite::Result<WithdrawalRow> {
    let state: String = r.get(15)?;
    let proof = match r.get::<_, Option<String>>(20)? {
        Some(document_id) => Some(ProvedDocument {
            document_id,
            height: get_opt_u64(r, 21)?.unwrap_or(0),
            block_time: get_opt_u64(r, 22)?,
        }),
        None => None,
    };
    let evidence = match (
        get_opt_u64(r, 23)?,
        get_opt_u64(r, 24)?,
        get_opt_u64(r, 25)?,
    ) {
        (Some(nonce_height), Some(identity_height), Some(document_height)) => {
            Some(NotExecutedEvidence {
                nonce_height,
                identity_height,
                document_height,
                reason: r.get(16)?,
            })
        }
        _ => None,
    };
    let mark = match r.get::<_, Option<String>>(28)? {
        Some(mark) => Some((
            WithdrawalMark::parse(&mark)?,
            get_opt_u64(r, 29)?.unwrap_or(0),
        )),
        None => None,
    };
    Ok(WithdrawalRow {
        signed: SignedWithdrawal {
            id: r.get(0)?,
            transition_hash: r.get(1)?,
            created_by: r.get(2)?,
            pro_tx_hash: r.get(3)?,
            identity_id: r.get(4)?,
            key_kind: WithdrawalKeyKind::parse(&r.get::<_, String>(5)?)?,
            amount: get_u64(r, 6)?,
            core_fee_per_byte: r.get(7)?,
            pooling: r.get(8)?,
            expected_duffs: get_u64(r, 9)?,
            nonce: get_u64(r, 10)?,
            signed_bytes: r.get(11)?,
            script: r.get(12)?,
            expected_script: r.get(13)?,
            signed_at: get_u64(r, 14)?,
        },
        state: WithdrawalState::parse(&state).ok_or_else(|| corrupt("state", &state))?,
        state_reason: r.get(16)?,
        sent_at: get_opt_u64(r, 17)?,
        paid_script: r.get(18)?,
        document_status: r.get(19)?,
        proof,
        evidence,
        floor_height: get_u64(r, 26)?,
        last_checked: get_opt_u64(r, 27)?,
        mark,
        late_proof_link: r.get(30)?,
    })
}

fn read_rows(
    conn: &Connection,
    filter: &str,
    args: impl rusqlite::Params,
) -> Result<Vec<WithdrawalRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM mn_withdrawals {filter} ORDER BY signed_at, rowid"
    ))?;
    let rows = stmt.query_map(args, read_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `Err(TransitionRefused)` unless every state of `from` may become `to`.
fn check_edges(from: &[WithdrawalState], to: WithdrawalState) -> Result<()> {
    match from.iter().find(|f| !f.may_become(to)) {
        Some(f) => Err(AppDbError::TransitionRefused {
            from: f.as_str(),
            to: to.as_str(),
        }),
        None => Ok(()),
    }
}

/// `('a', 'b')` for an `IN` over states (fixed strings, no input).
fn state_list(states: &[WithdrawalState]) -> String {
    let quoted: Vec<_> = states.iter().map(|s| format!("'{}'", s.as_str())).collect();
    format!("({})", quoted.join(", "))
}

/// The current state of record `id`, in the transaction.
fn state_of(conn: &Connection, id: &str) -> Result<Option<(WithdrawalState, String, u64, String)>> {
    let row: Option<(String, String, i64, String)> = conn
        .query_row(
            "SELECT state, identity_id, nonce, pro_tx_hash FROM mn_withdrawals WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    row.map(|(state, identity, nonce, pro_tx_hash)| {
        let state = WithdrawalState::parse(&state)
            .ok_or_else(|| AppDbError::Corrupt(format!("mn_withdrawals.state {state:?}")))?;
        Ok((
            state,
            identity,
            u64::try_from(nonce).unwrap_or(0),
            pro_tx_hash,
        ))
    })
    .transpose()
}

/// Inserts `row` as `signed` at its group's floor (at least `floor_height`),
/// unless a payout-change barrier of its masternode exists. The partial
/// unique index refusing it is [`AppDbError::WithdrawalPending`].
fn insert_signed(conn: &Connection, row: &SignedWithdrawal, floor_height: u64) -> Result<()> {
    let barrier: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM mn_payout_barriers WHERE pro_tx_hash = ?1)",
        [&row.pro_tx_hash],
        |r| r.get(0),
    )?;
    if barrier {
        return Err(AppDbError::PayoutChangePending(row.pro_tx_hash.clone()));
    }
    let inserted = conn.execute(
        &format!(
            "INSERT INTO mn_withdrawals ({COLUMNS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                     'signed', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
                     max(?16, COALESCE((SELECT max(floor_height) FROM mn_withdrawals
                                        WHERE identity_id = ?5 AND nonce = ?11), 0)),
                     NULL, NULL, NULL, NULL)"
        ),
        params![
            row.id,
            row.transition_hash,
            row.created_by,
            row.pro_tx_hash,
            row.identity_id,
            row.key_kind.as_str(),
            sql_u64(row.amount),
            row.core_fee_per_byte,
            row.pooling,
            sql_u64(row.expected_duffs),
            sql_u64(row.nonce),
            row.signed_bytes,
            row.script,
            row.expected_script,
            sql_u64(row.signed_at),
            sql_u64(floor_height),
        ],
    );
    match inserted {
        // The group carries one floor (P5): a sibling signed on a newer
        // verified read raises the others to it.
        Ok(_) => {
            conn.execute(
                "UPDATE mn_withdrawals SET floor_height = (SELECT floor_height
                     FROM mn_withdrawals WHERE id = ?3)
                 WHERE identity_id = ?1 AND nonce = ?2 AND floor_height < (SELECT
                     floor_height FROM mn_withdrawals WHERE id = ?3)",
                params![row.identity_id, sql_u64(row.nonce), row.id],
            )?;
            Ok(())
        }
        Err(rusqlite::Error::SqliteFailure(e, Some(msg)))
            if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                && msg == "UNIQUE constraint failed: mn_withdrawals.identity_id" =>
        {
            Err(AppDbError::WithdrawalPending(row.identity_id.clone()))
        }
        Err(e) => Err(e.into()),
    }
}

impl AppDb {
    /// Writes a new record as `signed` before any network step, in one
    /// synced transaction (MG-13 P5) that first checks no payout-change
    /// barrier of its masternode exists (`PayoutChangePending`, r4 n1). The
    /// record starts at its group's floor, at least `floor_height` (the
    /// height of the signing-time verified reads), and raises its siblings'
    /// to it (P5: one floor per group). A second open record of the
    /// identity, from any process, is refused by the unique index
    /// (`WithdrawalPending`, P6).
    pub fn insert_withdrawal_durable(
        &self,
        row: &SignedWithdrawal,
        floor_height: u64,
    ) -> Result<()> {
        self.durably(|conn| {
            let tx = write_transaction(conn)?;
            insert_signed(&tx, row, floor_height)?;
            fault_before(&tx, MN_WITHDRAW_INSERT)?;
            Ok(tx.commit()?)
        })?;
        self.fault_after(MN_WITHDRAW_INSERT)
    }

    /// Try again (MG-13 P15), one synced transaction: first the refused
    /// record `refused_id` moves to `superseded` (it must change exactly one
    /// row), then `sibling` is inserted as [`Self::insert_withdrawal_durable`]
    /// would (the barrier check included). SQLite checks the partial unique
    /// index per statement, so the order matters. `false`, with nothing
    /// written, when `refused_id` is gone or not `refused`. The sibling must
    /// be of the same identity, masternode and nonce (`Invalid`).
    pub fn supersede_and_insert_withdrawal_durable(
        &self,
        refused_id: &str,
        sibling: &SignedWithdrawal,
        floor_height: u64,
    ) -> Result<bool> {
        let applied = self.durably(|conn| {
            let tx = write_transaction(conn)?;
            let Some((state, identity, nonce, pro_tx_hash)) = state_of(&tx, refused_id)? else {
                return Ok(false);
            };
            if identity != sibling.identity_id
                || nonce != sibling.nonce
                || pro_tx_hash != sibling.pro_tx_hash
            {
                return Err(AppDbError::Invalid(format!(
                    "the sibling of {refused_id} is of another group"
                )));
            }
            if state != WithdrawalState::Refused {
                return Ok(false);
            }
            let superseded = tx.execute(
                "UPDATE mn_withdrawals SET state = 'superseded'
                 WHERE id = ?1 AND state = 'refused'",
                [refused_id],
            )?;
            if superseded != 1 {
                return Ok(false);
            }
            fault_before(&tx, MN_WITHDRAW_SUPERSEDE)?;
            insert_signed(&tx, sibling, floor_height)?;
            tx.commit()?;
            Ok(true)
        })?;
        self.fault_after(MN_WITHDRAW_SUPERSEDE)?;
        Ok(applied)
    }

    /// Marks a hand-off of record `id` as starting, synced, before its bytes
    /// can leave: `sent_at` is set unless an earlier attempt set it. Whether
    /// one had (a refusal then is "after an earlier send", P5); `None` when
    /// there is no such record.
    pub fn begin_withdrawal_send_durable(&self, id: &str, now: u64) -> Result<Option<bool>> {
        let prior = self.durably(|conn| {
            let tx = write_transaction(conn)?;
            let prior: Option<Option<i64>> = tx
                .query_row(
                    "SELECT sent_at FROM mn_withdrawals WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(prior) = prior else {
                return Ok(None);
            };
            if prior.is_none() {
                tx.execute(
                    "UPDATE mn_withdrawals SET sent_at = ?2 WHERE id = ?1 AND sent_at IS NULL",
                    params![id, sql_u64(now)],
                )?;
            }
            fault_before(&tx, MN_WITHDRAW_SENT_AT)?;
            tx.commit()?;
            Ok(Some(prior.is_some()))
        })?;
        self.fault_after(MN_WITHDRAW_SENT_AT)?;
        Ok(prior)
    }

    /// A single record's state change, synced, compare-and-set: record `id`
    /// moves to `to` only if its state is one of `from` (what the caller
    /// read), with `reason` as its `state_reason`. Whether it applied.
    ///
    /// Only `sent`, `refused` and `couldnt_confirm` are written here; the
    /// changes that involve the group or another write have their own
    /// method: `superseded` ([`Self::supersede_and_insert_withdrawal_durable`]),
    /// the marks ([`Self::mark_withdrawal_durable`]), `done` and
    /// `sibling_paid` ([`Self::finalize_withdrawal_group_durable`]),
    /// `not_executed` ([`Self::not_executed_withdrawal_group_durable`]). Any
    /// pair outside P6's table is `TransitionRefused`, nothing written.
    pub fn set_withdrawal_state_durable(
        &self,
        id: &str,
        from: &[WithdrawalState],
        to: WithdrawalState,
        reason: Option<&str>,
    ) -> Result<bool> {
        use WithdrawalState::*;
        check_edges(from, to)?;
        if !matches!(to, Sent | Refused | CouldntConfirm) {
            return Err(AppDbError::TransitionRefused {
                from: from.first().map_or("", |f| f.as_str()),
                to: to.as_str(),
            });
        }
        if from.is_empty() {
            return Ok(false);
        }
        let applied = self.durably(|conn| {
            let tx = write_transaction(conn)?;
            let applied = tx.execute(
                &format!(
                    "UPDATE mn_withdrawals SET state = ?2, state_reason = ?3
                     WHERE id = ?1 AND state IN {}",
                    state_list(from)
                ),
                params![id, to.as_str(), reason],
            )? == 1;
            fault_before(&tx, MN_WITHDRAW_STATE)?;
            tx.commit()?;
            Ok(applied)
        })?;
        self.fault_after(MN_WITHDRAW_STATE)?;
        Ok(applied)
    }

    /// Decides a nonce group by its proved document (MG-13 P7, P8 step 1),
    /// one synced transaction: record `id` becomes `done` with `proof` if its
    /// state is one of `from` (compare-and-set), every other undecided
    /// record of its (identity, nonce) `sibling_paid`, and every record of
    /// the group the user marked (P9) gets `id` as its `late_proof_link`,
    /// the marks read in the same transaction. Whether it applied; `false`
    /// writes nothing.
    pub fn finalize_withdrawal_group_durable(
        &self,
        id: &str,
        from: &[WithdrawalState],
        proof: &WithdrawalProof,
    ) -> Result<bool> {
        check_edges(from, WithdrawalState::Done)?;
        if from.is_empty() {
            return Ok(false);
        }
        let applied = self.durably(|conn| {
            let tx = write_transaction(conn)?;
            let Some((_, identity, nonce, _)) = state_of(&tx, id)? else {
                return Ok(false);
            };
            let done = tx.execute(
                &format!(
                    "UPDATE mn_withdrawals SET state = 'done', proof_document_id = ?2,
                         proof_height = ?3, proof_block_time = ?4, paid_script = ?5,
                         document_status = ?6
                     WHERE id = ?1 AND state IN {}",
                    state_list(from)
                ),
                params![
                    id,
                    proof.document_id,
                    sql_u64(proof.height),
                    sql_u64(proof.block_time),
                    proof.paid_script,
                    proof.document_status,
                ],
            )?;
            if done != 1 {
                return Ok(false);
            }
            tx.execute(
                &format!(
                    "UPDATE mn_withdrawals SET state = 'sibling_paid'
                     WHERE identity_id = ?1 AND nonce = ?2 AND id <> ?3
                       AND state NOT IN {FINAL_STATES}"
                ),
                params![identity, sql_u64(nonce), id],
            )?;
            tx.execute(
                "UPDATE mn_withdrawals SET late_proof_link = ?3
                 WHERE identity_id = ?1 AND nonce = ?2 AND user_mark IS NOT NULL",
                params![identity, sql_u64(nonce), id],
            )?;
            fault_before(&tx, MN_WITHDRAW_FINALIZE)?;
            tx.commit()?;
            Ok(true)
        })?;
        self.fault_after(MN_WITHDRAW_FINALIZE)?;
        Ok(applied)
    }

    /// Decides a whole nonce group `not_executed` on P8's proof, one synced
    /// transaction, compare-and-set over the group: it applies only if the
    /// group's records are exactly `expected` (ids and states, as the caller
    /// read them), none final. Every record gets `evidence`. Whether it
    /// applied; `false` writes nothing.
    pub fn not_executed_withdrawal_group_durable(
        &self,
        identity_id: &str,
        nonce: u64,
        expected: &[(&str, WithdrawalState)],
        evidence: &NotExecutedEvidence,
    ) -> Result<bool> {
        let states: Vec<_> = expected.iter().map(|(_, s)| *s).collect();
        check_edges(&states, WithdrawalState::NotExecuted)?;
        if expected.is_empty() {
            return Err(AppDbError::Invalid("an empty group".into()));
        }
        let mut want: Vec<(String, WithdrawalState)> = (expected.iter())
            .map(|(id, s)| ((*id).to_owned(), *s))
            .collect();
        want.sort_by(|a, b| a.0.cmp(&b.0));
        let applied = self.durably(|conn| {
            let tx = write_transaction(conn)?;
            let mut group: Vec<(String, WithdrawalState)> = Vec::new();
            {
                let mut stmt = tx.prepare(
                    "SELECT id, state FROM mn_withdrawals WHERE identity_id = ?1 AND nonce = ?2
                     ORDER BY id",
                )?;
                let rows = stmt.query_map(params![identity_id, sql_u64(nonce)], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?;
                for row in rows {
                    let (id, state) = row?;
                    let state = WithdrawalState::parse(&state).ok_or_else(|| {
                        AppDbError::Corrupt(format!("mn_withdrawals.state {state:?}"))
                    })?;
                    group.push((id, state));
                }
            }
            if group != want {
                return Ok(false);
            }
            tx.execute(
                &format!(
                    "UPDATE mn_withdrawals SET state = 'not_executed', state_reason = ?3,
                         evidence_nonce_height = ?4, evidence_identity_height = ?5,
                         evidence_document_height = ?6
                     WHERE identity_id = ?1 AND nonce = ?2 AND state NOT IN {FINAL_STATES}"
                ),
                params![
                    identity_id,
                    sql_u64(nonce),
                    evidence.reason,
                    sql_u64(evidence.nonce_height),
                    sql_u64(evidence.identity_height),
                    sql_u64(evidence.document_height),
                ],
            )?;
            fault_before(&tx, MN_WITHDRAW_NOT_EXECUTED)?;
            tx.commit()?;
            Ok(true)
        })?;
        self.fault_after(MN_WITHDRAW_NOT_EXECUTED)?;
        Ok(applied)
    }

    /// Raises the floor of the nonce group (identity, nonce) to at least
    /// `height`, synced, in one statement over every record of the group
    /// (MG-13 P5): each gets the group's highest floor or `height`, whichever
    /// is higher, so the floor only rises and every sibling carries it. The
    /// group's floor afterwards; `None` when there is no such group. Only
    /// SPV-verified reads may raise it; that is the caller's rule.
    pub fn raise_withdrawal_floor_durable(
        &self,
        identity_id: &str,
        nonce: u64,
        height: u64,
    ) -> Result<Option<u64>> {
        let floor = self.durably(|conn| {
            let tx = write_transaction(conn)?;
            let raised = tx.execute(
                "UPDATE mn_withdrawals SET floor_height = max(?3,
                     (SELECT max(floor_height) FROM mn_withdrawals
                      WHERE identity_id = ?1 AND nonce = ?2))
                 WHERE identity_id = ?1 AND nonce = ?2",
                params![identity_id, sql_u64(nonce), sql_u64(height)],
            )?;
            if raised == 0 {
                return Ok(None);
            }
            let floor: i64 = tx.query_row(
                "SELECT max(floor_height) FROM mn_withdrawals WHERE identity_id = ?1 AND nonce = ?2",
                params![identity_id, sql_u64(nonce)],
                |r| r.get(0),
            )?;
            fault_before(&tx, MN_WITHDRAW_FLOOR)?;
            tx.commit()?;
            Ok(Some(u64::try_from(floor).unwrap_or(0)))
        })?;
        self.fault_after(MN_WITHDRAW_FLOOR)?;
        Ok(floor)
    }

    /// The user's mark (MG-13 P9), synced, compare-and-set from
    /// `couldnt_confirm` only: the record moves to `marked_received` or
    /// `marked_not_received`, the mark and its time stored apart from any
    /// proof. Whether it applied.
    pub fn mark_withdrawal_durable(&self, id: &str, mark: WithdrawalMark, at: u64) -> Result<bool> {
        let applied = self.durably(|conn| {
            let tx = write_transaction(conn)?;
            let applied = tx.execute(
                "UPDATE mn_withdrawals SET state = ?2, user_mark = ?3, marked_at = ?4
                 WHERE id = ?1 AND state = 'couldnt_confirm'",
                params![id, mark.state().as_str(), mark.as_str(), sql_u64(at)],
            )? == 1;
            fault_before(&tx, MN_WITHDRAW_MARK)?;
            tx.commit()?;
            Ok(applied)
        })?;
        self.fault_after(MN_WITHDRAW_MARK)?;
        Ok(applied)
    }

    /// Notes what a recheck saw without changing the state (document status,
    /// the time of the check, a late proof's link, an open record's current
    /// reason), synced. `None` fields are kept. Whether the record exists.
    pub fn note_withdrawal_durable(&self, id: &str, note: &WithdrawalNote) -> Result<bool> {
        let found = self.durably(|conn| {
            let tx = write_transaction(conn)?;
            let found = tx.execute(
                "UPDATE mn_withdrawals SET
                     document_status = COALESCE(?2, document_status),
                     last_checked = COALESCE(?3, last_checked),
                     late_proof_link = COALESCE(?4, late_proof_link),
                     state_reason = COALESCE(?5, state_reason)
                 WHERE id = ?1",
                params![
                    id,
                    note.document_status,
                    note.last_checked.map(sql_u64),
                    note.late_proof_link,
                    note.state_reason,
                ],
            )? == 1;
            fault_before(&tx, MN_WITHDRAW_NOTE)?;
            tx.commit()?;
            Ok(found)
        })?;
        self.fault_after(MN_WITHDRAW_NOTE)?;
        Ok(found)
    }

    pub fn withdrawal(&self, id: &str) -> Result<Option<WithdrawalRow>> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT {COLUMNS} FROM mn_withdrawals WHERE id = ?1"),
                [id],
                read_row,
            )
            .optional()?)
    }

    /// The identity's records, oldest first.
    pub fn withdrawals_of_identity(&self, identity_id: &str) -> Result<Vec<WithdrawalRow>> {
        read_rows(&self.conn(), "WHERE identity_id = ?1", [identity_id])
    }

    /// The masternode's records, oldest first.
    pub fn withdrawals_of_masternode(&self, pro_tx_hash: &str) -> Result<Vec<WithdrawalRow>> {
        read_rows(&self.conn(), "WHERE pro_tx_hash = ?1", [pro_tx_hash])
    }

    /// The nonce group (identity, nonce), oldest first.
    pub fn withdrawal_group(&self, identity_id: &str, nonce: u64) -> Result<Vec<WithdrawalRow>> {
        read_rows(
            &self.conn(),
            "WHERE identity_id = ?1 AND nonce = ?2",
            params![identity_id, sql_u64(nonce)],
        )
    }

    /// The identity's open record (at most one, P6).
    pub fn open_withdrawal(&self, identity_id: &str) -> Result<Option<WithdrawalRow>> {
        let open: Vec<_> = (WithdrawalState::ALL.into_iter())
            .filter(|s| s.is_open())
            .collect();
        let filter = format!("WHERE identity_id = ?1 AND state IN {}", state_list(&open));
        Ok(read_rows(&self.conn(), &filter, [identity_id])?.pop())
    }

    /// Every undecided record (not final), app-wide, oldest first (P8).
    pub fn undecided_withdrawals(&self) -> Result<Vec<WithdrawalRow>> {
        read_rows(
            &self.conn(),
            &format!("WHERE state NOT IN {FINAL_STATES}"),
            [],
        )
    }
}

#[cfg(test)]
#[path = "mn_withdrawals_tests.rs"]
mod tests;
