-- Content-bound positive provenance (DEC-183). A verification record now
-- holds a digest of the entity as it verified, so data the library replaced
-- afterwards (from any source, the trusted-quorum fallback included) no
-- longer passes for verified: a reader counts a record only when the digest
-- of the entity it shows equals this one.
--   digest  `<encoding version>:<lowercase hex SHA-256>` of the entity's
--           canonical bytes (dw-engine `platform::reverify`; the version the
--           digest names, `v2` when this migration was written). NULL on
--           rows written before this migration: they read as unverified
--           until a re-fetch verifies the entity again.
ALTER TABLE dp_trust_verified ADD COLUMN digest TEXT;

-- Builds before E0-10b served every Platform proof with the trusted
-- service's quorum keys but never set the fallback latch, so their data
-- would read as verified and never be re-fetched (Opus-high M1). An
-- installation that already holds wallets starts latched: its data reads as
-- unverified until a fetch verifies it against SPV-held keys. A fresh
-- database has no wallets yet and is left alone; the engine latches it the
-- first time the fallback serves a read.
INSERT OR IGNORE INTO settings_kv (scope, key, value)
SELECT '', 'trust.fallback_used_at', CAST(strftime('%s', 'now') AS TEXT)
WHERE EXISTS (SELECT 1 FROM wallets);
