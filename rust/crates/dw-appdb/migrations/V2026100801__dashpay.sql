-- DashPay tables (DASHPAY §3.4). Append-only, like every migration here.
--
-- Wallet-scoped tables name the wallet `wallet_id` (64-char lowercase hex), as
-- the tables of the initial schema do, so wallet removal and the .dwbackup
-- row export (every table with a `wallet_id` column) cover them. Identities
-- and contacts are Platform identifiers in their base58 text form. Times are
-- UNIX seconds. Enumerated TEXT columns (`phase`, `kind`, `status`) carry no
-- CHECK: the engine owns the value set, and a later migration cannot widen a
-- CHECK without rebuilding the table.

-- The identity a wallet shows as its own: the one the identity chip, the
-- username and the contact list belong to.
CREATE TABLE dp_main_identity (
    wallet_id TEXT NOT NULL PRIMARY KEY,
    identity  TEXT NOT NULL
) STRICT;

-- One row per registration flow (the persisted state machine of §3.4),
-- advanced by the engine after each step.
--   identity_index       the identity key index, once keys are prepared
--   identity             the identity id, once registered (or the existing
--                        identity a flow adopts)
--   funding              how the flow is funded and its payload (wallet
--                        funds, an invitation link id, an existing identity);
--                        never a secret. A versioned, tagged JSON object;
--                        DASHPAY §3.4 lists what it must carry
--   initial_profile      the profile entered at Draft (a JSON object: display
--                        name, public message, avatar URL, avatar hash and
--                        fingerprint; nothing secret), kept until the
--                        ProfileCreated step so it survives a restart and a
--                        .dwbackup restore (`id` is renumbered by a restore, so
--                        it cannot key the profile elsewhere); NULL when the
--                        flow has none
--   asset_lock_outpoint  `<txid>:<vout>`, written when the asset lock is
--                        built, before it is broadcast. One asset lock funds
--                        one flow: unique per wallet (below)
--   phase                the state's name, e.g. 'draft', 'funding_sent'; a
--                        failed flow keeps the phase it failed in
--   error                the failure code: set exactly when the flow is Failed
--   retryable            1 when the failed flow may be resumed
--   updated_at           when the row last changed, not when the phase did;
--                        never the start of a wait (the InstantSend window of
--                        §2.6 runs from an in-memory instant)
CREATE TABLE dp_registration (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    wallet_id           TEXT    NOT NULL,
    identity_index      INTEGER CHECK (identity_index IS NULL OR identity_index >= 0),
    identity            TEXT,
    label               TEXT    NOT NULL,
    temp_label          TEXT,
    funding             TEXT    NOT NULL,
    initial_profile     TEXT,
    asset_lock_outpoint TEXT,
    phase               TEXT    NOT NULL,
    error               TEXT,
    retryable           INTEGER NOT NULL DEFAULT 0
                        CHECK (retryable IN (0, 1) AND (retryable = 0 OR error IS NOT NULL)),
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
) STRICT;
CREATE INDEX dp_registration_wallet ON dp_registration (wallet_id, phase);
-- Draft and KeysPrepared rows (no outpoint yet) are unaffected: NULLs are
-- distinct, and the index leaves them out. The uniqueness is byte-wise, so it
-- holds for one canonical text only: the lower-case hex txid in display order,
-- a colon, the decimal vout (`OutPoint`'s `Display`), e.g. 'ab12...ef:0'. The
-- column has no format CHECK; every writer must produce that text ('T:0' and
-- 't:0' would count as two flows). An upsert on this index must repeat the
-- WHERE clause below in its ON CONFLICT target.
CREATE UNIQUE INDEX dp_registration_lock ON dp_registration (wallet_id, asset_lock_outpoint)
    WHERE asset_lock_outpoint IS NOT NULL;

-- Own contested usernames being watched until the vote ends. Scoped to the
-- wallet (a wallet-less key would let one wallet's removal, or its backup,
-- touch another's rows) so removing a wallet, and its backup, cover it
-- exactly.
CREATE TABLE dp_contest_watch (
    wallet_id  TEXT    NOT NULL,
    identity   TEXT    NOT NULL,
    label      TEXT    NOT NULL,
    ends_at    INTEGER NOT NULL,
    last_state TEXT,
    PRIMARY KEY (wallet_id, identity, label)
) STRICT;

-- The notification journal (§3.5), written from classified changesets.
-- `contact` and `ref` (a txid or a contact-request id) are '' when the event
-- has none, so the UNIQUE key makes a journal write idempotent per
-- (kind, contact, ref) with INSERT OR IGNORE. An event that can recur for one
-- identity (a username, a contest outcome) must put something in `ref` that
-- differs between occurrences: the DPNS domain document id, or the label plus
-- the contest end. The label alone does not: a name that is sold and bought
-- back would be deduplicated away.
-- `read_at` is NULL while unread.
CREATE TABLE dp_events (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    wallet_id TEXT    NOT NULL,
    identity  TEXT    NOT NULL,
    kind      TEXT    NOT NULL,
    contact   TEXT    NOT NULL DEFAULT '',
    ref       TEXT    NOT NULL DEFAULT '',
    at        INTEGER NOT NULL,
    read_at   INTEGER,
    UNIQUE (wallet_id, identity, kind, contact, ref)
) STRICT;
CREATE INDEX dp_events_unread ON dp_events (wallet_id, identity) WHERE read_at IS NULL;

-- A payment to a contact whose outcome is unknown: the contact is locked
-- until the user resolves it. One lock per contact.
CREATE TABLE dp_payment_lock (
    wallet_id TEXT    NOT NULL,
    identity  TEXT    NOT NULL,
    contact   TEXT    NOT NULL,
    txid      TEXT    NOT NULL,
    since     INTEGER NOT NULL,
    PRIMARY KEY (wallet_id, identity, contact)
) STRICT;

-- Entities stored while the trusted-quorum fallback was in use (§2.2) and not
-- yet re-verified against SPV: `kind` is 'identity', 'contact_request' or
-- 'dpns_label'; `key` is its id or label. No money moves to an entity that
-- has a row. Scoped to the wallet like the other tables, but the stored
-- entity is not: wallet.sqlite keeps one row per identity (a write from
-- another wallet is a no-op), so wallet A's flag can cover an identity that
-- wallet B relies on. The money-move gate therefore blocks when ANY wallet
-- flags the entity (`WHERE kind = ? AND key = ?`, not just the current
-- wallet_id), and a verified re-fetch of an identity clears its rows in every
-- wallet. Contact requests and DPNS states are per wallet there, so those rows
-- are cleared by the owning wallet's re-fetch. Removing a wallet removes its
-- rows; a backup carries them (restoring a flag is the conservative choice).
CREATE TABLE dp_trust_unverified (
    wallet_id TEXT    NOT NULL,
    kind      TEXT    NOT NULL,
    key       TEXT    NOT NULL,
    since     INTEGER NOT NULL,
    PRIMARY KEY (wallet_id, kind, key)
) STRICT;

-- The avatar cache index, one row per avatar URL; the files are in
-- `<network>/avatars/`. The cache belongs to the network, not to a wallet.
--   url_sha      SHA-256 of the avatar URL (hex)
--   content_sha  SHA-256 of the fetched bytes (hex)
--   dhash        the perceptual hash, 16 hex digits
--   status       the outcome of fetching and decoding the URL, and only that
--                (e.g. 'ok', 'fetch_failed', 'not_an_image'). Checking the
--                content against a profile's `avatarHash` and fingerprint is
--                per profile, not per URL (two profiles can cite one URL with
--                different hashes), so it is done when reading, by comparing
--                `content_sha` and `dhash` with the profile, and never stored
--   file         the stem of the thumbnails' names in the avatars directory
--                (one file per size), derived from `content_sha`, so rows of
--                different URLs with the same content share files: the
--                rotation unlinks a file only when no other row names it, and
--                counts a shared file once
--   bytes        the thumbnails' size on disk, for the rotation
CREATE TABLE dp_avatar (
    url_sha     TEXT    NOT NULL PRIMARY KEY,
    content_sha TEXT,
    dhash       TEXT,
    status      TEXT    NOT NULL,
    file        TEXT,
    fetched_at  INTEGER NOT NULL,
    bytes       INTEGER CHECK (bytes IS NULL OR bytes >= 0)
) STRICT;
CREATE INDEX dp_avatar_fetched ON dp_avatar (fetched_at);

-- Per-identity preferences, scoped to the wallet like `dp_contest_watch`.
CREATE TABLE dp_prefs (
    wallet_id TEXT NOT NULL,
    identity  TEXT NOT NULL,
    key       TEXT NOT NULL,
    value     TEXT NOT NULL,
    PRIMARY KEY (wallet_id, identity, key)
) STRICT;
