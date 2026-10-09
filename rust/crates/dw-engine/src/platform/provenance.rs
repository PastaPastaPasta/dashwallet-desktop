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
//! provider in use, and nothing is unverified. The latch is durable before
//! the fallback serves anything (DEC-135): [`Provenance::fallback_in_use`]
//! writes it and the caller refuses the fallback when that fails, so a
//! reopened session can never forget it. In memory it only caches the
//! durable value, except that a latch that cannot be read counts as set.
//!
//! Reads are in memory (the facade's `identities()` is), loaded at open and
//! written through.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
    /// A DPNS label; the key is the homograph-normalized label.
    pub(crate) const DPNS_LABEL: &str = "dpns_label";
    /// A contact request; the key is `sender:recipient:$createdAt`.
    #[expect(dead_code, reason = "DP2's contact read models check it")]
    pub(crate) const CONTACT_REQUEST: &str = "contact_request";
    /// A DashPay payment; the key is the txid.
    #[expect(dead_code, reason = "DP3-04's money-move gate checks it")]
    pub(crate) const PAYMENT: &str = "payment";
}

/// The session's provenance: the fallback latch and the verified records.
pub(crate) struct Provenance {
    appdb: Arc<AppDb>,
    /// The latch as far as reads go: on file, or unreadable.
    fallback_used: AtomicBool,
    /// The latch is known to be on file (read back, or written here).
    latch_durable: AtomicBool,
    verified: Mutex<HashSet<(String, String)>>,
}

impl Provenance {
    /// Loads the latch and the records. A latch that cannot be read counts
    /// as set (fail closed). Blocking (app.sqlite).
    pub(crate) fn open(appdb: Arc<AppDb>) -> Self {
        let (fallback_used, latch_durable) = match appdb
            .setting(dw_appdb::GLOBAL_SCOPE, TRUST_FALLBACK_USED_KEY)
        {
            Ok(latch) => (latch.is_some(), latch.is_some()),
            Err(e) => {
                tracing::warn!(error = %e, "could not read the trust latch: treating data as unverified");
                (true, false)
            }
        };
        let verified = appdb.verified_entities().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not read the verification records");
            Vec::new()
        });
        Self {
            appdb,
            fallback_used: AtomicBool::new(fallback_used),
            latch_durable: AtomicBool::new(latch_durable),
            verified: Mutex::new(verified.into_iter().collect()),
        }
    }

    /// The fallback is about to serve a read (E0-10b): from now on, absence
    /// of a record means unverified. On `Err` the latch is not on file and
    /// the caller must not use the fallback for this fetch: it fails or
    /// degrades as if there were none. Idempotent, and concurrent first
    /// callers each return only once the write has committed. Blocking
    /// (app.sqlite) until the latch is known durable.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "E0-10b's quorum provider calls it")
    )]
    pub(crate) fn fallback_in_use(&self) -> Result<(), dw_appdb::AppDbError> {
        if self.latch_durable.load(Ordering::SeqCst) {
            return Ok(());
        }
        // The first write's time stays.
        (self.appdb).raise_setting(
            dw_appdb::GLOBAL_SCOPE,
            TRUST_FALLBACK_USED_KEY,
            unix_now(),
            true,
        )?;
        self.latch_durable.store(true, Ordering::SeqCst);
        self.fallback_used.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// One of the entity's fetches verified against SPV-held quorum keys
    /// (E0-10b). Blocking (app.sqlite).
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "E0-10b's quorum provider calls it")
    )]
    pub(crate) fn record_verified(
        &self,
        kind: &str,
        key: &str,
    ) -> Result<(), dw_appdb::AppDbError> {
        self.appdb.mark_verified(kind, key, unix_now())?;
        guard(&self.verified).insert((kind.to_string(), key.to_string()));
        Ok(())
    }

    /// Whether `(kind, key)` must be treated as unverified: the fallback has
    /// been used and no verification is on record. In memory.
    pub(crate) fn is_unverified(&self, kind: &str, key: &str) -> bool {
        self.fallback_used.load(Ordering::SeqCst)
            && !guard(&self.verified).contains(&(kind.to_string(), key.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Before the fallback is ever used nothing is unverified; once it is,
    /// everything without a record is, for good (a later session reads the
    /// latch back), until a verified fetch records it.
    #[test]
    fn absence_is_unverified_once_the_fallback_was_used() {
        let appdb = Arc::new(AppDb::open_in_memory().unwrap());
        let p = Provenance::open(Arc::clone(&appdb));
        assert!(!p.is_unverified(kind::IDENTITY, "a"));

        p.fallback_in_use().unwrap();
        assert!(p.is_unverified(kind::IDENTITY, "a"));
        assert!(p.is_unverified(kind::PROFILE, "a"));
        p.record_verified(kind::IDENTITY, "a").unwrap();
        assert!(!p.is_unverified(kind::IDENTITY, "a"));
        assert!(p.is_unverified(kind::PROFILE, "a"), "per kind");

        let later = Provenance::open(Arc::clone(&appdb));
        assert!(!later.is_unverified(kind::IDENTITY, "a"));
        assert!(later.is_unverified(kind::DPNS_LABEL, "a11ce"));
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
        assert!(!p.is_unverified(kind::IDENTITY, "a"));
        assert!(!open().is_unverified(kind::IDENTITY, "a"));

        p.fallback_in_use().unwrap();
        assert!(p.is_unverified(kind::IDENTITY, "a"));
        assert!(open().is_unverified(kind::IDENTITY, "a"), "after a reopen");
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
        assert!(p.is_unverified(kind::IDENTITY, "a"), "fails closed");

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
        assert!(reopened.is_unverified(kind::IDENTITY, "a"));
    }
}
