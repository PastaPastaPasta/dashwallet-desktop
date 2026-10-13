-- Wallets removed in this run (DEC-210). `AppDb::delete_wallet` records the
-- wallet in the transaction that deletes its rows, and from then on an
-- insert of a row of that wallet is skipped (`RAISE(IGNORE)`), so a write
-- that was already under way when the wallet was removed leaves nothing
-- behind. Every open of the database empties the table (no write outlives
-- the run that removed its wallet), and an import of the wallet takes its
-- row out before its first write. Not carried by a `.dwbackup` (dw-appdb
-- `rows.rs`).
CREATE TABLE removed_wallets (
    wallet_id TEXT PRIMARY KEY NOT NULL
) STRICT;

-- One trigger per table `delete_wallet` clears (`WALLET_TABLES`).
CREATE TRIGGER wallets_after_removal BEFORE INSERT ON wallets
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
CREATE TRIGGER address_book_after_removal BEFORE INSERT ON address_book
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
CREATE TRIGGER labels_after_removal BEFORE INSERT ON labels
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
CREATE TRIGGER tx_meta_after_removal BEFORE INSERT ON tx_meta
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
CREATE TRIGGER receive_requests_after_removal BEFORE INSERT ON receive_requests
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
CREATE TRIGGER utxo_locks_after_removal BEFORE INSERT ON utxo_locks
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
CREATE TRIGGER dp_main_identity_after_removal BEFORE INSERT ON dp_main_identity
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
CREATE TRIGGER dp_registration_after_removal BEFORE INSERT ON dp_registration
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
CREATE TRIGGER dp_contest_watch_after_removal BEFORE INSERT ON dp_contest_watch
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
CREATE TRIGGER dp_events_after_removal BEFORE INSERT ON dp_events
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
CREATE TRIGGER dp_payment_lock_after_removal BEFORE INSERT ON dp_payment_lock
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
CREATE TRIGGER dp_trust_unverified_after_removal BEFORE INSERT ON dp_trust_unverified
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
CREATE TRIGGER dp_prefs_after_removal BEFORE INSERT ON dp_prefs
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
CREATE TRIGGER dp_contact_info_after_removal BEFORE INSERT ON dp_contact_info
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;

-- Settings: the wallet's scope and its `local:` scope. The global scope
-- ('') is never a wallet id.
CREATE TRIGGER settings_kv_after_removal BEFORE INSERT ON settings_kv
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.scope)
    OR (NEW.scope GLOB 'local:*' AND EXISTS (
        SELECT 1 FROM removed_wallets WHERE wallet_id = substr(NEW.scope, 7)))
BEGIN SELECT RAISE(IGNORE); END;
