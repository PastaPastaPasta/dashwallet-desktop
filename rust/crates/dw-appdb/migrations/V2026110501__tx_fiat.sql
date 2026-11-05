-- The price a transaction was first seen at (MP-01, docs/contracts/rates-engine.md
-- §6): one row per transaction, written once and never changed, so a later
-- price or a currency switch leaves it as it was. Only a fresh price stamps,
-- only on Mainnet, and only a transaction first seen (or mined) at most 3 hours
-- earlier: a rescan or a restore never stamps today's price on an old payment.
-- There is no back-fill.
--   currency    ISO 4217 code the price is in.
--   price       fiat per 1 DASH, the engine's exact decimal string.
--   rate_at     UNIX seconds the price is from (its age anchor).
--   source      `ctx`, or `bitPayDashCentral` for the fallback.
--   stamped_at  UNIX seconds the row was written.
-- Carried by a `.dwbackup` like every `wallet_id` table (dw-appdb `rows.rs`).
CREATE TABLE tx_fiat (
    wallet_id TEXT NOT NULL,
    txid TEXT NOT NULL CHECK (length(txid) = 64 AND txid NOT GLOB '*[^0-9a-f]*'),
    currency TEXT NOT NULL CHECK (length(currency) = 3 AND currency NOT GLOB '*[^A-Z]*'),
    price TEXT NOT NULL CHECK (length(price) BETWEEN 1 AND 28 AND price NOT GLOB '*[^0-9.]*'),
    rate_at INTEGER NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('ctx', 'bitPayDashCentral')),
    stamped_at INTEGER NOT NULL,
    PRIMARY KEY (wallet_id, txid)
) STRICT;

-- DEC-210: a removed wallet's insert is skipped (`V2026101302`).
CREATE TRIGGER tx_fiat_after_removal BEFORE INSERT ON tx_fiat
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
