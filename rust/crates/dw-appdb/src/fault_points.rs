//! The names of dw-appdb's fault points (MG-13 §3 "Fault sweep"); the
//! mechanism is the `fault` module, compiled only in debug builds with
//! `fault-inject` (and tests). Each point has a `before_` and an `after_`
//! variant (`mn_withdraw.after_insert`).

/// The record's insert ([`crate::AppDb::insert_withdrawal_durable`]).
pub const MN_WITHDRAW_INSERT: &str = "mn_withdraw.insert";
/// `sent_at` before a hand-off ([`crate::AppDb::begin_withdrawal_send_durable`]).
pub const MN_WITHDRAW_SENT_AT: &str = "mn_withdraw.sent_at";
/// A single record's state write ([`crate::AppDb::set_withdrawal_state_durable`]).
pub const MN_WITHDRAW_STATE: &str = "mn_withdraw.state";
/// A group's `done` and `sibling_paid` ([`crate::AppDb::finalize_withdrawal_group_durable`]).
pub const MN_WITHDRAW_FINALIZE: &str = "mn_withdraw.finalize";
/// A group's `not_executed` ([`crate::AppDb::not_executed_withdrawal_group_durable`]).
pub const MN_WITHDRAW_NOT_EXECUTED: &str = "mn_withdraw.not_executed";
/// Try again's transaction ([`crate::AppDb::supersede_and_insert_withdrawal_durable`]);
/// its `before_` is hit between the supersede and the sibling's insert.
pub const MN_WITHDRAW_SUPERSEDE: &str = "mn_withdraw.supersede";
/// The user's mark ([`crate::AppDb::mark_withdrawal_durable`]).
pub const MN_WITHDRAW_MARK: &str = "mn_withdraw.mark";
/// A group's floor raise ([`crate::AppDb::raise_withdrawal_floor_durable`]).
pub const MN_WITHDRAW_FLOOR: &str = "mn_withdraw.floor";
/// Document status, last check, late-proof link ([`crate::AppDb::note_withdrawal_durable`]).
pub const MN_WITHDRAW_NOTE: &str = "mn_withdraw.note";
/// A barrier written with its operation's signed row (both op writers).
pub const MN_BARRIER_WRITE: &str = "mn_barrier.write";
/// A barrier cleared ([`crate::AppDb::clear_payout_barrier_durable`]).
pub const MN_BARRIER_CLEAR: &str = "mn_barrier.clear";
/// The discard's delete of an unsent operation and its barriers
/// ([`crate::AppDb::discard_unsent_masternode_op_durable`]).
pub const MN_BARRIER_DISCARD: &str = "mn_barrier.discard";
/// The upgrade's backfill ([`crate::AppDb::backfill_payout_barriers_durable`]).
pub const MN_BARRIER_BACKFILL: &str = "mn_barrier.backfill";
