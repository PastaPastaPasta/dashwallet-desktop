-- A contact payment from the Shielded or Platform balance (C-funding D-C5,
-- P6), written durably before its source record and before anything is
-- signed. While `pending` it is the pair's payment lock, as a
-- `dp_payment_lock` row is: one lock per pair across every source.
--   reference       the payout's id, assigned before the row: a Platform
--                   move's id, or an id the engine makes for a shielded spend.
--   source          `shielded` or `platform`.
--   st_hash         a shielded spend's transition hash (hex), written before
--                   its dispatch artifact; NULL until then and for Platform.
--   address         the contact's DIP-15 Core address it pays.
--   credits         what leaves the balance.
--   expected_duffs  what the contact receives on Core, about.
--   note            this wallet's own (M5); never on chain.
--   state           `pending`, `sent` or `not_sent`.
--   since           UNIX seconds the row was written.
-- Carried by a `.dwbackup` like every `wallet_id` table (dw-appdb `rows.rs`).
CREATE TABLE dp_contact_payout (
    wallet_id TEXT NOT NULL,
    reference TEXT NOT NULL CHECK (length(reference) BETWEEN 1 AND 128),
    identity TEXT NOT NULL,
    contact TEXT NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('shielded', 'platform')),
    st_hash TEXT CHECK (
        st_hash IS NULL OR (length(st_hash) = 64 AND st_hash NOT GLOB '*[^0-9a-f]*')
    ),
    address TEXT NOT NULL,
    credits INTEGER NOT NULL CHECK (credits > 0),
    expected_duffs INTEGER NOT NULL CHECK (expected_duffs >= 0),
    note TEXT,
    state TEXT NOT NULL CHECK (state IN ('pending', 'sent', 'not_sent')),
    since INTEGER NOT NULL,
    PRIMARY KEY (wallet_id, reference)
) STRICT;

CREATE UNIQUE INDEX dp_contact_payout_pending ON dp_contact_payout (wallet_id, identity, contact)
WHERE state = 'pending';

-- DEC-210: a removed wallet's insert is skipped (`V2026101302`).
CREATE TRIGGER dp_contact_payout_after_removal BEFORE INSERT ON dp_contact_payout
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
