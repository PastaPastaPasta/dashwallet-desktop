-- The wallet's shared-masternode sessions (MG-04b §9.1): one row per session
-- the wallet coordinates or takes part in. The engine derives each row's
-- state on read from these columns, the wallet's history and the masternode
-- list; it never stores one. A row whose signed bytes may have left the
-- wallet (`out_at` set) is never deleted: every delete is
-- `... AND out_at IS NULL` (MG-04b §9.2, MG-04 D6). The database is one per
-- network directory (`app.sqlite`), so a row carries no network column.
--   id            the session id, 64 lower-case hex.
--   kind          what the session does.
--   role          this wallet's part in it.
--   envelope      the latest canonical message JSON; public data only.
--   my_shares     JSON: share index -> keychain owner index.
--   contribution  JSON: this wallet's recorded inputs, their total and its
--                 change; NULL until it contributes.
--   reserved      JSON array of "txid:vout": the outpoints the wallet locked
--                 for the session (persistent locks, written with the row).
--   approved_terms  the terms the wallet last approved, JSON.
--   base_keys     a key change's base keys (D25), JSON.
--   signed_tx     this wallet's signed copy, consensus bytes; written before
--                 it is returned or handed off.
--   txid          the transaction's id, once known.
--   out_at        UNIX seconds; set when bytes that may be broadcast leave
--                 the wallet. Needs `signed_tx`.
--   abandon_txid  the transaction that spends a reserved input to abandon
--                 the session (§9.3).
-- Not carried by a `.dwbackup` (dw-appdb `rows.rs` `NOT_EXPORTED`, D19): the
-- reserved coins and locks are this installation's, and a sent session is in
-- the history a restore reads again.
CREATE TABLE shared_mn_sessions (
    id TEXT NOT NULL CHECK (length(id) = 64 AND id NOT GLOB '*[^0-9a-f]*'),
    wallet_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('registration', 'dissolve_together', 'key_change')),
    role TEXT NOT NULL CHECK (role IN ('coordinator', 'participant')),
    envelope TEXT NOT NULL,
    my_shares TEXT NOT NULL,
    contribution TEXT,
    reserved TEXT NOT NULL DEFAULT '[]',
    approved_terms TEXT,
    base_keys TEXT,
    signed_tx BLOB,
    txid TEXT,
    out_at INTEGER,
    abandon_txid TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_error TEXT,
    CHECK (out_at IS NULL OR signed_tx IS NOT NULL),
    PRIMARY KEY (wallet_id, id)
) STRICT;
CREATE INDEX shared_mn_sessions_wallet ON shared_mn_sessions (wallet_id, updated_at);

-- DEC-210: a removed wallet's insert is skipped (`V2026101302`).
CREATE TRIGGER shared_mn_sessions_after_removal BEFORE INSERT ON shared_mn_sessions
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
