//! A fork-written `shielded.sqlite` for other crates' tests: the fork's
//! tables, verbatim. Never enabled by the app.

use std::path::Path;

use rusqlite::{Connection, params};

/// The fork's `shielded_pending_spends`, verbatim (fork `file_store.rs`).
pub const FORK_PENDING: &str = "CREATE TABLE shielded_pending_spends (
    wallet_id     BLOB    NOT NULL,
    account_index INTEGER NOT NULL,
    activity_id   BLOB    NOT NULL,
    anchor        BLOB    NOT NULL,
    nullifiers    BLOB    NOT NULL,
    st_bytes      BLOB    NOT NULL,
    attempts      INTEGER NOT NULL DEFAULT 0,
    identity_nonce_finalized INTEGER NOT NULL DEFAULT 0,
    identity_user_abandoned INTEGER NOT NULL DEFAULT 0,
    unsent        INTEGER NOT NULL DEFAULT 0,
    armed_recording INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (wallet_id, account_index, activity_id)
)";

/// The fork's `shielded_spend_dispositions`, verbatim (fork `file_store.rs`).
pub const FORK_DISPOSITIONS: &str = "CREATE TABLE shielded_spend_dispositions (
    wallet_id     BLOB    NOT NULL
        CHECK (typeof(wallet_id) = 'blob' AND length(wallet_id) = 32),
    account_index INTEGER NOT NULL
        CHECK (typeof(account_index) = 'integer'
               AND account_index BETWEEN 0 AND 4294967295),
    st_hash       BLOB    NOT NULL
        CHECK (typeof(st_hash) = 'blob' AND length(st_hash) = 32),
    activity_id   BLOB    NOT NULL
        CHECK (typeof(activity_id) = 'blob' AND length(activity_id) = 32),
    disposition   TEXT    NOT NULL
        CHECK (disposition IN ('executed', 'not_executed', 'unresolved')),
    reason        TEXT
        CHECK ((disposition = 'executed') = (reason IS NULL)),
    at            INTEGER NOT NULL,
    nullifiers    BLOB    NOT NULL DEFAULT x''
        CHECK (typeof(nullifiers) = 'blob' AND length(nullifiers) % 32 = 0
               AND (disposition IN ('executed', 'unresolved')
                    OR length(nullifiers) = 0)),
    released_by_scan INTEGER NOT NULL DEFAULT 0
        CHECK (released_by_scan IN (0, 1)),
    PRIMARY KEY (wallet_id, account_index, st_hash)
);
CREATE UNIQUE INDEX IF NOT EXISTS shielded_spend_dispositions_by_transition
    ON shielded_spend_dispositions (wallet_id, st_hash);";

/// A row of a fork store ([`fork_store`]).
pub enum ForkRow {
    /// A spend disposition of transition and activity `[st; 32]`:
    /// `kind` `executed`, `not_executed` or `unresolved`, with `reason`
    /// (none for `executed`), holding `nullifiers`.
    Disposition {
        st: u8,
        kind: &'static str,
        reason: Option<&'static str>,
        nullifiers: Vec<[u8; 32]>,
    },
    /// A stored spend of activity `[activity; 32]`, recorded under the
    /// fork's hook (`armed_recording = 1`), never sent when `unsent`.
    Pending {
        activity: u8,
        nullifiers: Vec<[u8; 32]>,
        st_bytes: Vec<u8>,
        unsent: bool,
    },
}

/// Writes a fork store at `path`: the fork's two tables, with `rows` of
/// `wallet`'s account `account`.
pub fn fork_store(
    path: &Path,
    wallet: &[u8; 32],
    account: u32,
    rows: &[ForkRow],
) -> rusqlite::Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch(FORK_PENDING)?;
    conn.execute_batch(FORK_DISPOSITIONS)?;
    let list = |ns: &[[u8; 32]]| -> Vec<u8> { ns.iter().flatten().copied().collect() };
    for row in rows {
        match row {
            ForkRow::Disposition {
                st,
                kind,
                reason,
                nullifiers,
            } => conn.execute(
                "INSERT INTO shielded_spend_dispositions (wallet_id, account_index, st_hash, \
                 activity_id, disposition, reason, at, nullifiers) \
                 VALUES (?1, ?2, ?3, ?3, ?4, ?5, 1, ?6)",
                params![
                    &wallet[..],
                    account,
                    &[*st; 32][..],
                    kind,
                    reason,
                    list(nullifiers)
                ],
            )?,
            ForkRow::Pending {
                activity,
                nullifiers,
                st_bytes,
                unsent,
            } => conn.execute(
                "INSERT INTO shielded_pending_spends (wallet_id, account_index, activity_id, \
                 anchor, nullifiers, st_bytes, unsent, armed_recording) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)",
                params![
                    &wallet[..],
                    account,
                    &[*activity; 32][..],
                    &[0xee_u8; 32][..],
                    list(nullifiers),
                    st_bytes,
                    unsent
                ],
            )?,
        };
    }
    Ok(())
}
