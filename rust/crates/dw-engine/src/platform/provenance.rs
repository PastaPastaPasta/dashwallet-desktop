//! Positive provenance (DEC-125; DASHPAY §2.2 rules 2 and 5). Once the
//! trusted-quorum fallback has been used, a Platform-sourced entity is
//! unverified unless dw-appdb's `dp_trust_verified` has a record for it,
//! written when one of its fetches verified against SPV-held quorum keys
//! (E0-10b). Absence fails closed by construction: whatever the library keeps
//! in memory or persists, after a refused store or through a path nothing
//! taps, cannot pass for verified.
//!
//! The fallback's use is a latch (`TRUST_FALLBACK_USED_KEY`), not the
//! fallback's current state: data fetched through it stays unverified after
//! SPV syncs, until a verified re-fetch records it. Before the latch (no
//! fallback ever used here) every fetch was proof-verified against the
//! provider in use, and nothing is unverified. Builds before E0-10b served
//! the trusted service's keys without setting it, so dw-appdb migration
//! `trust_digest` sets it for an installation that already holds wallets:
//! their data starts unverified and is re-fetched. The latch is durable before
//! the fallback serves anything (DEC-135, DEC-139, DEC-143):
//! [`Provenance::fallback_in_use`] commits it and checkpoints the WAL (so it
//! survives power loss), and the caller refuses the fallback when either
//! fails, so a reopened session can never forget it. Durability is
//! established in each process, never inferred: a latch read back at open
//! counts for classification (as does one that cannot be read), but the
//! first fallback use still checkpoints.
//!
//! Reads are in memory (the facade's `identities()` is), loaded at open and
//! written through.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dw_appdb::AppDb;

use super::runtime::guard;
use crate::events::unix_now;

/// The global setting set when the trusted-quorum fallback is first used,
/// UNIX seconds. Never cleared by the engine.
pub(crate) const TRUST_FALLBACK_USED_KEY: &str = "trust.fallback_used_at";

/// `dp_trust_verified.kind` values (dw-appdb migration `trust_verified`).
pub(crate) mod kind {
    /// An identity and its keys; the key is the identity id (Base58).
    pub(crate) const IDENTITY: &str = "identity";
    /// An identity's DashPay profile; the key is the identity id.
    pub(crate) const PROFILE: &str = "profile";
    /// A DPNS label; the key is the homograph-normalized label, the digest
    /// binds the spelling shown and the owner. Records live in the network's
    /// own app.sqlite, so a key needs no network.
    pub(crate) const DPNS_LABEL: &str = "dpns_label";
    /// A contact request; the key is [`super::contact_request_key`].
    pub(crate) const CONTACT_REQUEST: &str = "contact_request";
    /// A DashPay payment; the key is the txid.
    #[expect(
        dead_code,
        reason = "no consumer: DP3-04's gate checks the payee's entities (its D11)"
    )]
    pub(crate) const PAYMENT: &str = "payment";
}

/// The `dp_trust_verified` key of a contact request: the sender's and the
/// recipient's Base58 ids and its `$createdAt` in ms, joined by `:` (the
/// library's `ContactRequest` carries no document id). The contacts read
/// model checks it; E0-10b writes it.
pub(crate) fn contact_request_key(
    sender: &Identifier,
    recipient: &Identifier,
    created_at: u64,
) -> String {
    let b58 = |id: &Identifier| id.to_string(Encoding::Base58);
    format!("{}:{}:{created_at}", b58(sender), b58(recipient))
}

/// An entity as provenance sees it: its record's kind and key, and the
/// digest of its content as the library holds it now (DEC-183).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Entity {
    pub(crate) kind: &'static str,
    pub(crate) key: String,
    /// `None` if the content could not be encoded: it never verifies.
    pub(crate) digest: Option<String>,
}

/// The session's provenance: the fallback latch and the verified records.
pub(crate) struct Provenance {
    appdb: Arc<AppDb>,
    /// The latch as far as reads go: on file, or unreadable.
    fallback_used: AtomicBool,
    /// The latch is power-loss durable: a full checkpoint after it
    /// succeeded in this process (DEC-143). Never inferred from a row read
    /// back, which may be a committed but unsynced WAL frame.
    latch_durable: AtomicBool,
    /// `(kind, key)` → the digest of the content that verified.
    verified: Mutex<HashMap<(String, String), String>>,
    /// Contact → the DPNS label the read models show for it, as spelled
    /// ([`Self::show_label`]). In memory.
    shown_labels: Mutex<HashMap<Identifier, String>>,
}

impl Provenance {
    /// Loads the latch and the records. A latch that cannot be read counts
    /// as set (fail closed). Blocking (app.sqlite).
    pub(crate) fn open(appdb: Arc<AppDb>) -> Self {
        let fallback_used = match appdb.setting(dw_appdb::GLOBAL_SCOPE, TRUST_FALLBACK_USED_KEY) {
            Ok(latch) => latch.is_some(),
            Err(e) => {
                tracing::warn!(error = %e, "could not read the trust latch: treating data as unverified");
                true
            }
        };
        let verified = appdb.verified_entities().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not read the verification records");
            Vec::new()
        });
        Self {
            appdb,
            fallback_used: AtomicBool::new(fallback_used),
            latch_durable: AtomicBool::new(false),
            verified: Mutex::new(
                (verified.into_iter())
                    .map(|(kind, key, digest)| ((kind, key), digest))
                    .collect(),
            ),
            shown_labels: Mutex::default(),
        }
    }

    /// The fallback is about to serve a read (E0-10b): from now on, absence
    /// of a record means unverified. On `Err` the latch is not on file and
    /// the caller must not use the fallback for this fetch: it fails or
    /// degrades as if there were none. Idempotent, and concurrent first
    /// callers each return only once the write has committed and been
    /// checkpointed. Each process checkpoints once before its first `Ok`,
    /// even over a latch already on file (DEC-143): a row left by a refused
    /// checkpoint is readable but not durable. Blocking (app.sqlite) until
    /// then.
    pub(crate) fn fallback_in_use(&self) -> Result<(), dw_appdb::AppDbError> {
        if self.latch_durable.load(Ordering::SeqCst) {
            return Ok(());
        }
        // The first write's time stays. Then power-loss durable (DEC-139):
        // a committed write under `synchronous=NORMAL` is not yet synced.
        (self.appdb).raise_setting(
            dw_appdb::GLOBAL_SCOPE,
            TRUST_FALLBACK_USED_KEY,
            unix_now(),
            true,
        )?;
        self.appdb.checkpoint_full()?;
        // Classification first: a caller taking the fast path below must
        // never find the latch durable but not yet counted.
        self.fallback_used.store(true, Ordering::SeqCst);
        self.latch_durable.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// A fetch of `entity`, with this content, verified against SPV-held
    /// quorum keys (E0-10b). An entity without a digest records nothing.
    /// Blocking (app.sqlite).
    pub(crate) fn record_verified(&self, entity: &Entity) -> Result<(), dw_appdb::AppDbError> {
        let Some(digest) = &entity.digest else {
            return Ok(());
        };
        (self.appdb).mark_verified(entity.kind, &entity.key, digest, unix_now())?;
        let key = (entity.kind.to_string(), entity.key.clone());
        guard(&self.verified).insert(key, digest.clone());
        Ok(())
    }

    /// The read models show `label` as `contact`'s DPNS name (`None`: no
    /// name). platform-wallet keeps no contact names, so the re-fetch
    /// verifies the shown ones from here, with `contact` as their owner.
    pub(crate) fn show_label(&self, contact: Identifier, label: Option<&str>) {
        let mut shown = guard(&self.shown_labels);
        match label {
            Some(l) => shown.insert(contact, l.to_string()),
            None => shown.remove(&contact),
        };
    }

    /// The contact labels shown, as spelled, each with its contact.
    pub(super) fn shown_labels(&self) -> Vec<(Identifier, String)> {
        (guard(&self.shown_labels).iter())
            .map(|(c, l)| (*c, l.clone()))
            .collect()
    }

    /// Whether the fallback was ever used (the latch, DEC-125).
    pub(crate) fn fallback_used(&self) -> bool {
        self.fallback_used.load(Ordering::SeqCst)
    }

    /// Whether `entity` must be treated as unverified: the fallback has been
    /// used and no verification of this content is on record (DEC-183: a
    /// record of other content does not count). In memory.
    pub(crate) fn is_unverified(&self, entity: &Entity) -> bool {
        self.fallback_used.load(Ordering::SeqCst) && {
            let key = (entity.kind.to_string(), entity.key.clone());
            let verified = guard(&self.verified);
            entity.digest.is_none() || verified.get(&key) != entity.digest.as_ref()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(kind: &'static str, key: &str, digest: &str) -> Entity {
        Entity {
            kind,
            key: key.to_string(),
            digest: Some(digest.to_string()),
        }
    }

    /// Before the fallback is ever used nothing is unverified; once it is,
    /// everything without a record is, for good (a later session reads the
    /// latch back), until a verified fetch records it.
    #[test]
    fn absence_is_unverified_once_the_fallback_was_used() {
        let appdb = Arc::new(AppDb::open_in_memory().unwrap());
        let p = Provenance::open(Arc::clone(&appdb));
        assert!(!p.is_unverified(&e(kind::IDENTITY, "a", "v1:aa")));

        p.fallback_in_use().unwrap();
        assert!(p.is_unverified(&e(kind::IDENTITY, "a", "v1:aa")));
        assert!(p.is_unverified(&e(kind::PROFILE, "a", "v1:aa")));
        p.record_verified(&e(kind::IDENTITY, "a", "v1:aa")).unwrap();
        assert!(!p.is_unverified(&e(kind::IDENTITY, "a", "v1:aa")));
        assert!(p.is_unverified(&e(kind::PROFILE, "a", "v1:aa")), "per kind");

        let later = Provenance::open(Arc::clone(&appdb));
        assert!(!later.is_unverified(&e(kind::IDENTITY, "a", "v1:aa")));
        assert!(later.is_unverified(&e(kind::DPNS_LABEL, "a11ce", "v1:aa")));
    }

    /// DEC-183: a record verifies the content it was written for. The entity
    /// replaced afterwards reads as unverified, in this session and the next,
    /// until its new content verifies; content that cannot be encoded never
    /// verifies.
    #[test]
    fn a_record_verifies_only_the_content_it_was_written_for() {
        let appdb = Arc::new(AppDb::open_in_memory().unwrap());
        let p = Provenance::open(Arc::clone(&appdb));
        p.fallback_in_use().unwrap();
        p.record_verified(&e(kind::IDENTITY, "a", "v1:aa")).unwrap();
        assert!(
            p.is_unverified(&e(kind::IDENTITY, "a", "v1:bb")),
            "replaced"
        );
        assert!(Provenance::open(Arc::clone(&appdb)).is_unverified(&e(
            kind::IDENTITY,
            "a",
            "v1:bb"
        )));

        p.record_verified(&e(kind::IDENTITY, "a", "v1:bb")).unwrap();
        assert!(
            !p.is_unverified(&e(kind::IDENTITY, "a", "v1:bb")),
            "re-verified"
        );
        assert!(
            p.is_unverified(&e(kind::IDENTITY, "a", "v1:aa")),
            "the old content"
        );
        assert!(!Provenance::open(Arc::clone(&appdb)).is_unverified(&e(
            kind::IDENTITY,
            "a",
            "v1:bb"
        )));

        let unencodable = Entity {
            digest: None,
            ..e(kind::IDENTITY, "c", "")
        };
        p.record_verified(&unencodable).unwrap();
        assert!(p.is_unverified(&unencodable));
    }

    /// Sol r3's busy-write probe (DEC-135): a latch write that fails (another
    /// writer holds app.sqlite) refuses the fallback and leaves memory as the
    /// disk: nothing was fetched through it, and a reopen agrees. The next
    /// call writes it, and a reopened session keeps it.
    #[test]
    fn a_latch_that_cannot_be_written_refuses_the_fallback() {
        let dir = dw_testutil::private_tempdir();
        let path = dir.path().join(dw_appdb::APP_DB_FILE);
        let open = || Provenance::open(Arc::new(AppDb::open(&path).unwrap()));
        let p = open();
        let holder = rusqlite::Connection::open(&path).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert!(p.fallback_in_use().is_err(), "the fallback is refused");
        assert!(p.fallback_in_use().is_err(), "and again, not skipped");
        holder.execute_batch("ROLLBACK").unwrap();
        assert!(!p.is_unverified(&e(kind::IDENTITY, "a", "v1:aa")));
        assert!(!open().is_unverified(&e(kind::IDENTITY, "a", "v1:aa")));

        p.fallback_in_use().unwrap();
        assert!(p.is_unverified(&e(kind::IDENTITY, "a", "v1:aa")));
        assert!(
            open().is_unverified(&e(kind::IDENTITY, "a", "v1:aa")),
            "after a reopen"
        );
    }

    /// DEC-139: the latch is power-loss durable before `fallback_in_use`
    /// returns. A copy of the database file alone (what survives when the
    /// unsynced WAL does not) has it.
    #[test]
    fn the_latch_is_in_the_database_file_before_the_fallback_is_used() {
        let dir = dw_testutil::private_tempdir();
        let path = dir.path().join(dw_appdb::APP_DB_FILE);
        let p = Provenance::open(Arc::new(AppDb::open(&path).unwrap()));
        p.fallback_in_use().unwrap();
        let copy = dir.path().join("file-alone.sqlite");
        std::fs::copy(&path, &copy).unwrap();
        let latch: Option<String> = rusqlite::Connection::open(&copy)
            .unwrap()
            .query_row(
                "SELECT value FROM settings_kv WHERE scope = ?1 AND key = ?2",
                [dw_appdb::GLOBAL_SCOPE, TRUST_FALLBACK_USED_KEY],
                |r| r.get(0),
            )
            .ok();
        assert!(latch.is_some(), "the latch is only in the WAL");
    }

    /// Sol r3's read-error probe: a latch that cannot be read counts as set,
    /// but that inference is not durability. Once the database is readable
    /// again the fallback's first use still writes it, so a reopen keeps it.
    #[test]
    fn a_latch_inferred_from_a_read_error_is_still_written() {
        let dir = dw_testutil::private_tempdir();
        let path = dir.path().join(dw_appdb::APP_DB_FILE);
        let appdb = Arc::new(AppDb::open(&path).unwrap());
        let raw = rusqlite::Connection::open(&path).unwrap();
        raw.execute_batch("DROP TABLE settings_kv").unwrap();
        let p = Provenance::open(Arc::clone(&appdb));
        assert!(
            p.is_unverified(&e(kind::IDENTITY, "a", "v1:aa")),
            "fails closed"
        );

        raw.execute_batch(
            "CREATE TABLE settings_kv (
                 scope TEXT NOT NULL,
                 key   TEXT NOT NULL,
                 value TEXT NOT NULL,
                 PRIMARY KEY (scope, key)
             ) STRICT",
        )
        .unwrap();
        p.fallback_in_use().unwrap();
        let reopened = Provenance::open(appdb);
        assert!(reopened.is_unverified(&e(kind::IDENTITY, "a", "v1:aa")));
    }
}
