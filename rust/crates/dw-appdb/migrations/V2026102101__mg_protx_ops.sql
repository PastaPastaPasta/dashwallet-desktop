-- The wallet's ProTx operations (MG-04 §5): one row per ProRegTx or
-- ProUpRegTx the wallet prepared or signed. The engine derives each row's
-- state on read from these columns and the wallet's history; it never
-- stores one. A row with signed bytes that may have been handed to the
-- network (`sent_at` set) is never deleted: every delete is
-- `... AND sent_at IS NULL` (MG-06 r1 M-1, MG-04 D6).
CREATE TABLE masternode_ops (
    id TEXT NOT NULL,
    wallet_id TEXT NOT NULL,
    -- 'registration' | 'update_registrar'
    kind TEXT NOT NULL,
    -- `ProTxTerms` and the request it was built from, as JSON; no secret.
    terms TEXT NOT NULL,
    request TEXT NOT NULL,
    -- External, prepared: the unsigned transaction (empty `vchSig`), the
    -- address and message the collateral key signs, the fee inputs locked
    -- for it (JSON array of "txid:vout").
    unsigned_tx BLOB,
    sign_address TEXT,
    sign_message TEXT,
    fee_inputs TEXT NOT NULL DEFAULT '[]',
    -- Written before any hand-off (save before broadcast).
    signed_tx BLOB,
    txid TEXT,
    pro_tx_hash TEXT,
    -- This row locked an ExistingUtxo collateral (MG-04 D9).
    locked_collateral INTEGER NOT NULL DEFAULT 0,
    fee INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    signed_at INTEGER,
    -- Set before each hand-off that may put the bytes out.
    sent_at INTEGER,
    last_error TEXT,
    CHECK (sent_at IS NULL OR signed_tx IS NOT NULL),
    CHECK ((signed_tx IS NULL) = (txid IS NULL)),
    PRIMARY KEY (wallet_id, id)
) STRICT;
CREATE INDEX masternode_ops_wallet ON masternode_ops (wallet_id, created_at);
CREATE TRIGGER masternode_ops_after_removal BEFORE INSERT ON masternode_ops
WHEN EXISTS (SELECT 1 FROM removed_wallets WHERE wallet_id = NEW.wallet_id)
BEGIN SELECT RAISE(IGNORE); END;
