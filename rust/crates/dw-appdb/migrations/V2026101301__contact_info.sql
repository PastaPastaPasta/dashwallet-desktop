-- What Platform holds of an identity's private details for one contact
-- (DP2-04, m4 §7 item 42): the digest of the alias, note and hidden flag of
-- the contact's `contactInfo` document as the engine last wrote or read it
-- back. A contact's details read as published when the digest of the
-- details shown equals this one. No plaintext: the details themselves are
-- the library's (`wallet.sqlite`).
--   digest       SHA-256 (32 bytes) of the details' DIP-15 encoding
--                (dw-engine `platform::private_details`).
--   recorded_at  UNIX seconds of the write or read that recorded it.
-- Derived from the network: not carried by a `.dwbackup` (dw-appdb
-- `rows.rs`); a restore records it again from its own read.
CREATE TABLE dp_contact_info (
    wallet_id   TEXT    NOT NULL,
    identity    TEXT    NOT NULL,
    contact     TEXT    NOT NULL,
    digest      BLOB    NOT NULL CHECK (length(digest) = 32),
    recorded_at INTEGER NOT NULL,
    PRIMARY KEY (wallet_id, identity, contact)
) STRICT;
