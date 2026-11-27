CREATE TABLE core_dropped_unsigned_spends (
    id INTEGER PRIMARY KEY,
    wallet_id BLOB NOT NULL,
    txid BLOB NOT NULL,
    record_blob BLOB NOT NULL,
    islock_blob BLOB NULL,
    islock_verified INTEGER NULL,
    deleted_outputs TEXT NOT NULL,
    released_coins TEXT NOT NULL,
    dropped_at INTEGER NOT NULL DEFAULT (unixepoch()),
    FOREIGN KEY (wallet_id) REFERENCES wallets(wallet_id) ON DELETE CASCADE
);

CREATE INDEX idx_core_dropped_unsigned_spends_txid
    ON core_dropped_unsigned_spends(wallet_id, txid);