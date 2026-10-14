-- The locked payment's inputs (DP3-01 D20), comma-separated `txid:vout`: a
-- ChainLocked transaction spending one of them proves the payment unsent,
-- also after a restart that lost the wallet's record of it.
ALTER TABLE dp_payment_lock ADD COLUMN inputs TEXT NOT NULL DEFAULT '';
