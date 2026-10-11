-- Content-bound positive provenance (DEC-183). A verification record now
-- holds a digest of the entity as it verified, so data the library replaced
-- afterwards (from any source, the trusted-quorum fallback included) no
-- longer passes for verified: a reader counts a record only when the digest
-- of the entity it shows equals this one.
--   digest  `<encoding version>:<lowercase hex SHA-256>` of the entity's
--           canonical bytes (dw-engine `platform::reverify`, encoding `v1`).
--           NULL on rows written before this migration: they read as
--           unverified until a re-fetch verifies the entity again.
ALTER TABLE dp_trust_verified ADD COLUMN digest TEXT;
