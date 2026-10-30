-- The wallet's standbys (MG-04b §9.1, §9.4): dash-qt's pair of signed
-- unilateral dissolutions of a shared masternode, kept to be sent later, one
-- row per actor share and fee. Rows are only added to or completed, never
-- changed: a stored variant is never replaced. The database is one per
-- network directory (`app.sqlite`), so a row carries no network column.
--   pro_tx_hash      the masternode, 64 lower-case hex as displayed.
--   share_index      the actor share, 0-based.
--   fee              duffs; both variants pay it.
--   full_tx          the full-principal variant (penalty 0), consensus bytes.
--   immediate_tx     the immediate variant (the early penalty paid); NULL
--                    when the penalty is 0 (the two are one transaction,
--                    m-20), or when only the other was imported.
--   full_valid_from  the first height the full variant is valid at: the
--                    registration's height plus the early period.
--   source           `created` by this wallet, or `imported` from a paste.
-- Stored standbys are plaintext signed transactions, as Core's file is:
-- anyone who reads this database can dissolve the masternode with one, so
-- the engine hands them out only under a grant (D21).
-- Not carried by a `.dwbackup` (dw-appdb `rows.rs` `NOT_EXPORTED`, D19): the
-- exported file is the backup.
CREATE TABLE shared_mn_standby (
    wallet_id TEXT NOT NULL,
    pro_tx_hash TEXT NOT NULL
        CHECK (length(pro_tx_hash) = 64 AND pro_tx_hash NOT GLOB '*[^0-9a-f]*'),
    share_index INTEGER NOT NULL CHECK (share_index BETWEEN 0 AND 7),
    fee INTEGER NOT NULL CHECK (fee > 0),
    full_tx BLOB,
    immediate_tx BLOB,
    full_valid_from INTEGER NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('created', 'imported')),
    created_at INTEGER NOT NULL,
    CHECK (full_tx IS NOT NULL OR immediate_tx IS NOT NULL),
    PRIMARY KEY (wallet_id, pro_tx_hash, share_index, fee)
) STRICT;

-- DEC-210: a removed wallet's insert is skipped (`V2026101302`).
CREATE TRIGGER shared_mn_standby_after_removal BEFORE INSERT ON shared_mn_standby
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
