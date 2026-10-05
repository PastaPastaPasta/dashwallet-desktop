-- dw-appdb initial schema. Migrations are append-only: never edit a shipped
-- file; add a new V<yyyymmddnn>__<name>.sql instead.
--
-- Every wallet-scoped table is keyed by the 64-char lowercase hex wallet id.
-- Txids are 64-char lowercase hex in display (RPC) order. Times are UNIX
-- seconds.

-- Wallet display names (E1 reads them into memory for the sync wallet list).
CREATE TABLE wallets (
    wallet_id  TEXT    NOT NULL PRIMARY KEY,
    name       TEXT    NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

-- dash-qt address book membership: which addresses appear on the sending
-- ('send') and receiving ('receive') pages. The label lives in `labels`
-- (kind 'address'), so an address has one label wherever it is shown.
CREATE TABLE address_book (
    wallet_id  TEXT    NOT NULL,
    address    TEXT    NOT NULL,
    purpose    TEXT    NOT NULL CHECK (purpose IN ('send', 'receive')),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (wallet_id, address)
) STRICT;

-- User labels of addresses and transactions.
CREATE TABLE labels (
    wallet_id  TEXT    NOT NULL,
    kind       TEXT    NOT NULL CHECK (kind IN ('address', 'tx')),
    target     TEXT    NOT NULL,
    label      TEXT    NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (wallet_id, kind, target)
) STRICT;

-- Local transaction metadata. `message` is the `message=` of the payment
-- URI(s) the transaction paid (dash-qt keeps it in the order form).
CREATE TABLE tx_meta (
    wallet_id  TEXT    NOT NULL,
    txid       TEXT    NOT NULL,
    message    TEXT,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (wallet_id, txid)
) STRICT;

-- Payment requests created on the Receive page (QT-081..083).
CREATE TABLE receive_requests (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    wallet_id  TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    address    TEXT    NOT NULL,
    amount     INTEGER CHECK (amount IS NULL OR amount > 0),
    label      TEXT,
    message    TEXT
) STRICT;
CREATE INDEX receive_requests_wallet ON receive_requests (wallet_id, created_at);

-- Outpoints excluded from automatic coin selection. 'manual' rows come from
-- "Lock unspent" and are deleted on unlock. 'dust' rows come from dust
-- protection; unlocking one sets `released_at` so the coin is not locked
-- again.
CREATE TABLE utxo_locks (
    wallet_id   TEXT    NOT NULL,
    txid        TEXT    NOT NULL,
    vout        INTEGER NOT NULL CHECK (vout >= 0),
    reason      TEXT    NOT NULL CHECK (reason IN ('manual', 'dust')),
    created_at  INTEGER NOT NULL,
    released_at INTEGER,
    PRIMARY KEY (wallet_id, txid, vout)
) STRICT;

-- Engine settings. `scope` is '' for network-wide values or a wallet id.
CREATE TABLE settings_kv (
    scope TEXT NOT NULL,
    key   TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (scope, key)
) STRICT;
