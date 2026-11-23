-- Positive provenance (DEC-125, DASHPAY §2.2 rule 5). One row per Platform
-- entity whose latest fetch verified against SPV-held quorum keys, written by
-- the layered quorum provider (E0-10b). Once the trusted-quorum fallback has
-- been used, an entity without a row here is unverified: absence fails
-- closed, so nothing the library keeps in memory or persists after a refused
-- or missed write can pass for verified.
--
-- Verification is a fact about Platform data, not about a wallet, so the
-- table is global (like `dp_avatar`): wallet removal leaves it, and a
-- .dwbackup does not carry it (a restored wallet re-verifies).
--   kind         'identity' (the identity and its keys), 'profile',
--                'contact_request', 'dpns_label' or 'payment'
--   key          the identity id (identity, profile); `sender:recipient:
--                $createdAt` (contact_request); the homograph-normalized
--                label (dpns_label); the txid (payment)
--   verified_at  when it last verified, UNIX seconds
CREATE TABLE dp_trust_verified (
    kind        TEXT    NOT NULL,
    key         TEXT    NOT NULL,
    verified_at INTEGER NOT NULL,
    PRIMARY KEY (kind, key)
) STRICT;
