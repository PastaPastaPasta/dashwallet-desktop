-- The wallet's own governance proposals (MG-06 §2.4, D4), saved before the
-- fee transaction is broadcast: Core's order (`rpc/governance.cpp`) makes the
-- object first and the fee second, and a fee sent with no object kept
-- anywhere cannot be turned into a proposal again.
--   hash          the object hash, wire order.
--   revision      the object's revision; the parent is always zero.
--   time          the object's time, UNIX seconds.
--   data          the proposal JSON bytes.
--   fee_txid      the fee transaction's txid, wire order.
--   fee_tx        the signed fee transaction, consensus bytes (a resend
--                 needs no signature).
--   created_at    UNIX seconds the row was saved.
--   fee_sent_at   NULL until the hand-off to the network was entered.
--   submitted_at  NULL until a peer fetched the object.
--   last_error    the last failure's code, if any.
-- Carried by a `.dwbackup` (dw-appdb `rows.rs` exports every table with a
-- `wallet_id` column): MG-00 D18, losing the row burns the 1 DASH fee.
CREATE TABLE mg_proposals (
    wallet_id    TEXT    NOT NULL,
    hash         BLOB    NOT NULL CHECK (length(hash) = 32),
    revision     INTEGER NOT NULL,
    time         INTEGER NOT NULL,
    data         BLOB    NOT NULL,
    fee_txid     BLOB    NOT NULL CHECK (length(fee_txid) = 32),
    fee_tx       BLOB    NOT NULL,
    created_at   INTEGER NOT NULL,
    fee_sent_at  INTEGER,
    submitted_at INTEGER,
    last_error   TEXT,
    PRIMARY KEY (wallet_id, hash)
) STRICT;

-- DEC-210: a removed wallet's insert is skipped (`V2026101302`).
CREATE TRIGGER mg_proposals_after_removal BEFORE INSERT ON mg_proposals
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;

-- The proposals a wallet's removal keeps (MG-06 D23): those whose fee is
-- paid and that have not expired, as a removal keeps the dispatch records of
-- a possibly sent asset lock (DP1-02 §3.1). Not a wallet table: it outlives
-- the removal, and the wallet's readmission copies its rows back. The last
-- removal's choice replaces an earlier one's.
CREATE TABLE mg_proposals_kept (
    wallet_id    TEXT    NOT NULL,
    hash         BLOB    NOT NULL CHECK (length(hash) = 32),
    revision     INTEGER NOT NULL,
    time         INTEGER NOT NULL,
    data         BLOB    NOT NULL,
    fee_txid     BLOB    NOT NULL CHECK (length(fee_txid) = 32),
    fee_tx       BLOB    NOT NULL,
    created_at   INTEGER NOT NULL,
    fee_sent_at  INTEGER,
    submitted_at INTEGER,
    last_error   TEXT,
    PRIMARY KEY (wallet_id, hash)
) STRICT;
