CREATE TABLE core_address_pool_retired (
    wallet_id BLOB NOT NULL,
    account_type TEXT NOT NULL,
    account_index INTEGER NOT NULL,
    key_class INTEGER NOT NULL DEFAULT 0,
    user_identity_id BLOB NOT NULL DEFAULT (zeroblob(32)),
    friend_identity_id BLOB NOT NULL DEFAULT (zeroblob(32)),
    -- The BIP32 serialization of the xpub these rows were derived from.
    account_xpub BLOB NOT NULL,
    pool_type INTEGER NOT NULL CHECK (pool_type IN (0, 1, 2, 3)),
    address_index INTEGER NOT NULL,
    script BLOB NOT NULL,
    used INTEGER NOT NULL DEFAULT 0 CHECK (used IN (0, 1)),
    public_key BLOB NULL,
    key_type INTEGER NULL CHECK (key_type IS NULL OR key_type IN (0, 1, 2)),
    retired_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (wallet_id, account_type, account_index, key_class, user_identity_id,
                 friend_identity_id, account_xpub, pool_type, address_index),
    FOREIGN KEY (wallet_id) REFERENCES wallets(wallet_id) ON DELETE CASCADE
);

CREATE INDEX idx_core_address_pool_retired_script
    ON core_address_pool_retired(wallet_id, script);

ALTER TABLE contacts ADD COLUMN external_account_reference INTEGER NULL
    CHECK (external_account_reference IS NULL
           OR external_account_reference BETWEEN 0 AND 4294967295);