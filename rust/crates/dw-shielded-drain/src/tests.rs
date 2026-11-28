//! Fixtures are built here from the fork's own DDL (fork `file_store.rs`,
//! `shielded_pending_spends` and `shielded_spend_dispositions` with their
//! CHECKs and index), beside a real journal, a real pre-upgrade snapshot and
//! a real persister open.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use dw_appdb::dispatch::{DISPATCH_DB_FILE, DispatchJournal, JournalOpen, ShieldedDrainState};
use dw_store_upgrade::test_support::{BeforeSnapshot, between_state_with};
use dw_store_upgrade::{Adopters, PersisterOpened, upgrade_copy_present};
use platform_wallet_storage::{LoadPolicy, SqlitePersister, SqlitePersisterConfig};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

use super::*;

const W: [u8; 32] = [0xa1; 32];

use crate::test_support::{FORK_DISPOSITIONS, FORK_PENDING};

/// The same table with every CHECK dropped (the F6 and corrupt-key cases).
const FORK_DISPOSITIONS_UNCHECKED: &str = "CREATE TABLE shielded_spend_dispositions (
    wallet_id BLOB NOT NULL, account_index INTEGER NOT NULL, st_hash BLOB NOT NULL,
    activity_id BLOB NOT NULL, disposition TEXT NOT NULL, reason TEXT, at INTEGER NOT NULL,
    nullifiers BLOB NOT NULL DEFAULT x'', released_by_scan INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (wallet_id, account_index, st_hash)
)";

fn private_dir(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}

/// A data root with one network directory.
struct Dir {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    net: PathBuf,
    persister: Option<(Arc<SqlitePersister>, PersisterOpened)>,
}

impl Dir {
    fn new() -> Self {
        let tmp = tempfile::Builder::new().prefix("dwsd-").tempdir().unwrap();
        let root = tmp.path().join("data");
        let net = root.join("testnet");
        private_dir(&root);
        private_dir(&net);
        Self {
            _tmp: tmp,
            root,
            net,
            persister: None,
        }
    }

    fn store_path(&self) -> PathBuf {
        self.net.join(SHIELDED_STORE_FILE)
    }

    /// A fork store: the pending table, and the dispositions table unless
    /// `dispositions` is `None`.
    fn fork_store(&self, dispositions: Option<&str>) -> Connection {
        let conn = Connection::open(self.store_path()).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.execute_batch(FORK_PENDING).unwrap();
        if let Some(ddl) = dispositions {
            conn.execute_batch(ddl).unwrap();
        }
        conn
    }

    fn store(&self) -> Connection {
        Connection::open(self.store_path()).unwrap()
    }

    /// The journal as a separate connection, for fixture rows and reads.
    fn journal_sql(&self) -> Connection {
        Connection::open(self.net.join(DISPATCH_DB_FILE)).unwrap()
    }

    fn journal(&self) -> DispatchJournal {
        match DispatchJournal::open(&self.net.join(DISPATCH_DB_FILE), 1).unwrap() {
            JournalOpen::Ready(j) => j,
            JournalOpen::NewerSchema(v) => panic!("newer schema {v}"),
        }
    }

    /// This upgrade's between state around the stores built so far: the
    /// store upgrade's fork fixture, with this directory's shielded store in
    /// place before its verified before-upgrade snapshot and this
    /// directory's journal after it, then this start's persister open.
    fn snapshot_and_open(&mut self) {
        let net = self.root.join("regtest");
        let moved = |from: &Path, to: &Path, file: &str| -> std::io::Result<()> {
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let name = format!("{file}{suffix}");
                if to.join(&name).exists() {
                    std::fs::remove_file(to.join(&name))?;
                }
                if from.join(&name).exists() {
                    std::fs::rename(from.join(&name), to.join(&name))?;
                }
            }
            Ok(())
        };
        between_state_with(
            &net,
            BeforeSnapshot::Present,
            false,
            SystemTime::now(),
            |dir| moved(&self.net, dir, SHIELDED_STORE_FILE),
        )
        .unwrap();
        moved(&self.net, &net, DISPATCH_DB_FILE).unwrap();
        assert!(dw_store_upgrade::snapshot_present(&net).unwrap());
        self.net = net;
        self.open_persister();
    }

    fn open_persister(&mut self) {
        Adopters::for_build(&self.root);
        let config = SqlitePersisterConfig::new(self.net.join("wallet.sqlite"))
            .with_load_policy(LoadPolicy::Strict)
            .with_auto_backup_dir(Some(self.net.join("backups").join("auto")));
        self.persister = Some(PersisterOpened::open(config).unwrap());
    }

    fn opened(&self) -> &PersisterOpened {
        &self.persister.as_ref().expect("persister open").1
    }

    fn drain(&self, fault: Option<FailAfter>) -> Result<DrainOutcome> {
        let journal = self.journal();
        run(&self.net, self.opened(), &journal, fault)
    }

    fn sha256s(&self) -> Vec<(String, Option<String>)> {
        [
            "wallet.sqlite",
            "shielded.sqlite",
            DISPATCH_DB_FILE,
            "app.sqlite",
        ]
        .iter()
        .map(|name| {
            let hash = std::fs::read(self.net.join(name))
                .ok()
                .map(|b| format!("{:x}", Sha256::digest(b)));
            (name.to_string(), hash)
        })
        .collect()
    }
}

fn n(b: u8) -> [u8; 32] {
    [b; 32]
}

fn list(ns: &[u8]) -> Vec<u8> {
    ns.iter().flat_map(|b| n(*b)).collect()
}

fn disposition(
    conn: &Connection,
    st: u8,
    kind: &str,
    reason: Option<&str>,
    held: &[u8],
    released: bool,
) {
    conn.execute(
        "INSERT INTO shielded_spend_dispositions (wallet_id, account_index, st_hash, \
         activity_id, disposition, reason, at, nullifiers) VALUES (?1, 0, ?2, ?3, ?4, ?5, 1, ?6)",
        params![&W[..], &n(st)[..], &n(st)[..], kind, reason, list(held)],
    )
    .unwrap();
    if released {
        conn.execute(
            "UPDATE shielded_spend_dispositions SET released_by_scan = 1 WHERE st_hash = ?1",
            params![&n(st)[..]],
        )
        .unwrap();
    }
}

fn pending(conn: &Connection, activity: u8, nullifiers: &[u8], st_bytes: &[u8], unsent: bool) {
    conn.execute(
        "INSERT INTO shielded_pending_spends (wallet_id, account_index, activity_id, anchor, \
         nullifiers, st_bytes, unsent, armed_recording) VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, 1)",
        params![
            &W[..],
            &n(activity)[..],
            &n(0xee)[..],
            list(nullifiers),
            st_bytes,
            unsent
        ],
    )
    .unwrap();
}

fn artifact(j: &Connection, activity: u8, st_hash: [u8; 32], origin: &str, status: &str) {
    let (kind, anchor) = if origin == "dispatched" {
        (Some("transfer"), Some(n(0xee).to_vec()))
    } else {
        (None, None)
    };
    j.execute(
        "INSERT INTO shielded_artifact (wallet, activity, origin, st_hash, kind, anchor, status, at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)",
        params![&W[..], &n(activity)[..], origin, &st_hash[..], kind, anchor, status],
    )
    .unwrap();
}

fn artifact_note(j: &Connection, activity: u8, nullifier: u8, spent: bool) {
    j.execute(
        "INSERT INTO shielded_artifact_note (wallet, activity, nullifier, spent) VALUES (?1, ?2, ?3, ?4)",
        params![&W[..], &n(activity)[..], &n(nullifier)[..], spent],
    )
    .unwrap();
}

fn status(
    j: &Connection,
    activity: u8,
) -> (String, Option<String>, Option<String>, Option<String>) {
    j.query_row(
        "SELECT status, status_reason, disposition_kind, disposition_reason FROM shielded_artifact \
         WHERE wallet = ?1 AND activity = ?2",
        params![&W[..], &n(activity)[..]],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .unwrap()
}

fn st(s: &str) -> (String, Option<String>, Option<String>, Option<String>) {
    let mut parts = s.split('/');
    let status = parts.next().unwrap().to_owned();
    let kind = parts.next().filter(|p| !p.is_empty()).map(str::to_owned);
    let reason = parts.next().map(str::to_owned);
    (status, None, kind, reason)
}

/// Every drained hold row: (st_hash byte, nullifier byte, origin, kind,
/// reason, spent).
type HoldRow = (u8, u8, String, String, Option<String>, bool);

fn holds(j: &Connection) -> Vec<HoldRow> {
    let mut stmt = j
        .prepare(
            "SELECT st_hash, nullifier, origin, kind, reason, spent FROM shielded_drained_hold \
             ORDER BY st_hash, nullifier",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok((
            r.get::<_, Vec<u8>>(0)?[0],
            r.get::<_, Vec<u8>>(1)?[0],
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
        ))
    })
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

fn unreadable(j: &Connection) -> Vec<(u8, String, Option<String>)> {
    let mut stmt = j
        .prepare("SELECT st_hash, kind, reason FROM shielded_drained_unreadable ORDER BY st_hash")
        .unwrap();
    stmt.query_map([], |r| {
        Ok((r.get::<_, Vec<u8>>(0)?[0], r.get(1)?, r.get(2)?))
    })
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

/// Everything the drain may change, as text: the journal's drain tables
/// and artifacts, and the store's tables and rows.
fn dump(d: &Dir) -> String {
    let j = d.journal_sql();
    let mut out = format!("holds {:?}\nunreadable {:?}\n", holds(&j), unreadable(&j));
    let mut stmt = j
        .prepare(
            "SELECT activity, status, status_reason, disposition_kind, disposition_reason \
             FROM shielded_artifact ORDER BY activity",
        )
        .unwrap();
    let rows: Vec<String> = stmt
        .query_map([], |r| {
            Ok(format!(
                "artifact {} {} {:?} {:?} {:?}\n",
                r.get::<_, Vec<u8>>(0)?[0],
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    out += &rows.concat();
    let marker: Option<String> = j
        .query_row("SELECT progress FROM shielded_drain", [], |r| r.get(0))
        .optional()
        .unwrap();
    out += &format!("marker {marker:?}\n");
    let s = d.store();
    out += &format!("tables {:?}\n", tables(&s));
    for t in [LIVE, RETIRED] {
        if tables(&s).contains(t) {
            out += &format!("{t} {:?}\n", fork_rows(&s, t));
        }
    }
    out += &format!("pending {:?}\n", pending_activities(&s));
    out
}

fn tables(s: &Connection) -> BTreeSet<String> {
    let mut stmt = s
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn fork_rows(s: &Connection, table: &str) -> Vec<(u8, String, Option<String>, i64)> {
    let mut stmt = s
        .prepare(&format!(
            "SELECT st_hash, disposition, reason, length(nullifiers) FROM {table} ORDER BY st_hash"
        ))
        .unwrap();
    stmt.query_map([], |r| {
        Ok((r.get::<_, Vec<u8>>(0)?[0], r.get(1)?, r.get(2)?, r.get(3)?))
    })
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

fn pending_activities(s: &Connection) -> Vec<u8> {
    let mut stmt = s
        .prepare("SELECT activity_id FROM shielded_pending_spends ORDER BY activity_id")
        .unwrap();
    stmt.query_map([], |r| Ok(r.get::<_, Vec<u8>>(0)?[0]))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

use rusqlite::OptionalExtension;

/// The fork rows of DRAIN rev 4's mapping table, one of each, over the
/// artifacts the journal holds for them.
fn mapping_fixture() -> Dir {
    let mut d = Dir::new();
    let s = d.fork_store(Some(FORK_DISPOSITIONS));
    disposition(&s, 1, "executed", None, &[1, 2], false);
    disposition(&s, 2, "executed", None, &[3], false);
    for (st, r) in [
        (10, "refused"),
        (11, "abandoned"),
        (12, "rejected"),
        (13, "pruned"),
    ] {
        disposition(&s, st, "not_executed", Some(r), &[], false);
    }
    for (st, r) in [
        (20, "deserialize_dropped"),
        (21, "purged"),
        (22, "unattributable"),
        (23, "cancelled"),
    ] {
        disposition(&s, st, "unresolved", Some(r), &[st], false);
    }
    disposition(&s, 30, "unresolved", Some("later"), &[30], false);
    disposition(&s, 31, "unresolved", Some("cancelled"), &[31], false);
    disposition(&s, 32, "not_executed", Some("refused"), &[], false);
    disposition(&s, 33, "not_executed", Some("later"), &[], false);
    disposition(&s, 34, "executed", None, &[34, 35], false);
    disposition(&s, 36, "executed", None, &[36], true);
    disposition(&s, 37, "executed", None, &[37], false);
    disposition(&s, 38, "unresolved", Some("cancelled"), &[38], false);
    drop(s);
    drop(d.journal());
    let j = d.journal_sql();
    artifact(&j, 1, n(1), "dispatched", "registered");
    for st in [10, 11, 12, 13] {
        artifact(&j, st, n(st), "dispatched", "registered");
    }
    for st in [20, 21, 22, 23] {
        artifact(&j, st, n(st), "dispatched", "unclassified");
    }
    artifact(&j, 30, n(30), "dispatched", "registered");
    artifact(&j, 31, n(31), "dispatched", "executed");
    artifact(&j, 32, n(32), "dispatched", "unresolved");
    artifact_note(&j, 32, 32, false);
    artifact(&j, 33, n(33), "dispatched", "registered");
    artifact(&j, 34, n(34), "dispatched", "registered");
    artifact_note(&j, 34, 34, true);
    artifact_note(&j, 34, 35, false);
    artifact(&j, 36, n(36), "dispatched", "registered");
    artifact(&j, 37, n(37), "imported_redrive", "unclassified");
    artifact(&j, 38, n(38), "imported_redrive", "unclassified");
    drop(j);
    d.snapshot_and_open();
    d
}

fn hold(st: u8, nf: u8, origin: &str, kind: &str, reason: Option<&str>, spent: bool) -> HoldRow {
    (
        st,
        nf,
        origin.into(),
        kind.into(),
        reason.map(Into::into),
        spent,
    )
}

/// DRAIN rev 4 test 1: each fork row lands in the journal as the mapping
/// table says (F1, F3, F4, C1, C3).
#[test]
fn every_fork_disposition_maps_to_its_journal_state_and_hold() {
    let d = mapping_fixture();
    let outcome = d.drain(None).unwrap();
    assert_eq!(
        outcome,
        DrainOutcome::Drained(DrainReport {
            dispositions: 18,
            held_notes: 14,
            unreadable: 2,
            never_sent: 0,
        })
    );
    let j = d.journal_sql();
    assert_eq!(status(&j, 1), st("executed/executed"));
    for (a, r) in [
        (10, "refused"),
        (11, "abandoned"),
        (12, "rejected"),
        (13, "pruned"),
    ] {
        assert_eq!(status(&j, a), st(&format!("not_executed/not_executed/{r}")));
    }
    for (a, r) in [
        (20, "deserialize_dropped"),
        (21, "purged"),
        (22, "unattributable"),
        (23, "cancelled"),
    ] {
        assert_eq!(status(&j, a), st(&format!("unresolved/unresolved/{r}")));
    }
    assert_eq!(status(&j, 30), st("unresolved"), "F3: no disposition");
    assert_eq!(status(&j, 31), st("executed"), "F4: executed stays");
    assert_eq!(
        status(&j, 32),
        st("unresolved"),
        "F4: not_executed over unresolved"
    );
    assert_eq!(status(&j, 33), st("unresolved"), "C3: no disposition");
    assert_eq!(status(&j, 34), st("executed/executed"));
    assert_eq!(status(&j, 36), st("executed/executed"));
    assert_eq!(status(&j, 37), st("executed/executed"), "F1");
    assert_eq!(status(&j, 38), st("unresolved/unresolved/cancelled"), "F1");
    assert_eq!(
        holds(&j),
        vec![
            hold(1, 1, "executed", "executed", None, false),
            hold(1, 2, "executed", "executed", None, false),
            hold(2, 3, "executed", "executed", None, false),
            hold(
                20,
                20,
                "unresolved",
                "unresolved",
                Some("deserialize_dropped"),
                false
            ),
            hold(21, 21, "unresolved", "unresolved", Some("purged"), false),
            hold(
                22,
                22,
                "unresolved",
                "unresolved",
                Some("unattributable"),
                false
            ),
            hold(23, 23, "unresolved", "unresolved", Some("cancelled"), false),
            hold(30, 30, "unreadable", "unresolved", Some("later"), false),
            hold(31, 31, "unresolved", "unresolved", Some("cancelled"), false),
            hold(34, 34, "executed", "executed", None, true),
            hold(34, 35, "executed", "executed", None, false),
            hold(36, 36, "executed", "executed", None, false),
            hold(37, 37, "executed", "executed", None, false),
            hold(38, 38, "unresolved", "unresolved", Some("cancelled"), false),
        ],
        "every executed, unresolved and unreadable held list, verbatim; C1 seeds 34's spent note"
    );
    assert_eq!(
        unreadable(&j),
        vec![
            (30, "unresolved".into(), Some("later".into())),
            (33, "not_executed".into(), Some("later".into())),
        ]
    );
    let journal = d.journal();
    for named in [1, 2, 30, 33, 37] {
        assert!(
            journal.shielded_drain_names(&W, &n(named)).unwrap(),
            "{named}"
        );
    }
    assert!(!journal.shielded_drain_names(&W, &n(10)).unwrap());
    let held: BTreeSet<u8> = journal
        .shielded_drained_holds(&W)
        .unwrap()
        .into_iter()
        .map(|(_, nf)| nf[0])
        .collect();
    assert_eq!(
        held,
        [1, 2, 3, 20, 21, 22, 23, 30, 31, 35, 36, 37, 38]
            .into_iter()
            .collect(),
        "C1: the note the journal read spent is not held"
    );
    assert_eq!(
        journal.shielded_drain_state(SHIELDED_STORE_FILE).unwrap(),
        Some(ShieldedDrainState::Done)
    );
    let s = d.store();
    assert!(!tables(&s).contains(LIVE));
    assert_eq!(
        fork_rows(&s, RETIRED).len(),
        18,
        "every row retired as it was"
    );
}

/// DRAIN rev 4 test 3 (journal and store halves): a never-sent record is
/// deleted and its artifact settled `not_executed`/`abandoned`; kept
/// records and an identity debit stay. Also the detection row "no
/// dispositions table, some unsent rows".
#[test]
fn a_never_sent_record_is_retired_and_its_notes_freed() {
    let mut d = Dir::new();
    let s = d.fork_store(None);
    pending(&s, 1, &[1], b"never sent", true);
    pending(&s, 2, &[2], b"sent under the hook", false);
    pending(&s, 3, &[], b"identity debit", true);
    pending(&s, 4, &[4], b"hookless", false);
    s.execute(
        "UPDATE shielded_pending_spends SET armed_recording = 0 WHERE activity_id = ?1",
        params![&n(4)[..]],
    )
    .unwrap();
    drop(s);
    drop(d.journal());
    let j = d.journal_sql();
    let st_hash: [u8; 32] = Sha256::digest(b"never sent").into();
    artifact(&j, 1, st_hash, "dispatched", "registered");
    artifact(&j, 2, n(2), "dispatched", "registered");
    drop(j);
    d.snapshot_and_open();
    assert_eq!(
        d.drain(None).unwrap(),
        DrainOutcome::Drained(DrainReport {
            never_sent: 1,
            ..DrainReport::default()
        })
    );
    let j = d.journal_sql();
    assert_eq!(status(&j, 1), st("not_executed/not_executed/abandoned"));
    assert_eq!(status(&j, 2), st("registered"));
    assert!(holds(&j).is_empty());
    let s = d.store();
    assert_eq!(pending_activities(&s), vec![2, 3, 4]);
    assert!(!tables(&s).contains(RETIRED));
    assert_eq!(d.drain(None).unwrap(), DrainOutcome::NotForkFormat);
}

/// DRAIN rev 4 test 5: a drain stopped after each step resumes to the end
/// state of an uninterrupted one, and holds throughout.
#[test]
fn a_crash_after_each_step_resumes_to_the_same_state() {
    let clean = mapping_fixture();
    clean.drain(None).unwrap();
    let want = dump(&clean);
    for fault in [FailAfter::Read, FailAfter::Journal, FailAfter::Retire] {
        let d = mapping_fixture();
        assert!(d.drain(Some(fault)).is_err(), "{fault:?}");
        let j = d.journal_sql();
        let s = d.store();
        match fault {
            FailAfter::Read => {
                assert!(holds(&j).is_empty());
                assert!(tables(&s).contains(LIVE), "the fork rows still hold");
            }
            FailAfter::Journal => {
                assert_eq!(holds(&j).len(), 14);
                assert!(tables(&s).contains(LIVE));
            }
            FailAfter::Retire => {
                assert_eq!(holds(&j).len(), 14, "the holds were in before the retire");
                assert!(!tables(&s).contains(LIVE));
            }
        }
        drop((j, s));
        let resumed = d.drain(None).unwrap();
        if fault == FailAfter::Retire {
            assert_eq!(resumed, DrainOutcome::Finished);
        } else {
            assert!(matches!(resumed, DrainOutcome::Drained(_)), "{fault:?}");
        }
        assert_eq!(dump(&d), want, "{fault:?}");
    }
}

/// F5: a drained store beside a lost journal is drained again from its
/// retired table, never read as not fork format.
#[test]
fn a_retired_store_beside_a_lost_journal_drains_again() {
    let d = mapping_fixture();
    d.drain(None).unwrap();
    let want = dump(&d);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(d.net.join(format!("{DISPATCH_DB_FILE}{suffix}")));
    }
    drop(d.journal());
    let j = d.journal_sql();
    artifact(&j, 1, n(1), "dispatched", "registered");
    drop(j);
    assert!(matches!(d.drain(None).unwrap(), DrainOutcome::Drained(_)));
    let j = d.journal_sql();
    assert_eq!(holds(&j).len(), 14, "every hold is back");
    assert_eq!(status(&j, 1), st("executed/executed"));
    drop(j);
    let store_part = |s: &str| s[s.find("tables").unwrap()..].to_owned();
    assert_eq!(
        store_part(&dump(&d)),
        store_part(&want),
        "the store is as it was"
    );
}

/// The fork's table as builds before `released_by_scan` created it.
fn fork_dispositions_without_scan_release() -> String {
    let column = "    released_by_scan INTEGER NOT NULL DEFAULT 0
        CHECK (released_by_scan IN (0, 1)),
";
    assert!(FORK_DISPOSITIONS.contains(column));
    FORK_DISPOSITIONS.replace(column, "")
}

/// F5: a fork build that ran after a drain recreated the live table (here
/// one without `released_by_scan`); both tables drain, the live row wins on
/// a shared key, and the live table goes. A `done` marker never skips it, a
/// nullifier an earlier drained hold reads spent is drained spent (C1), and
/// an unsent row written after the retire drains too.
#[test]
fn a_store_with_both_tables_drains_both() {
    let d = mapping_fixture();
    d.drain(None).unwrap();
    d.journal().note_spent_nullifiers(&W, &[n(3)]).unwrap();
    let s = d.store();
    s.execute_batch(&fork_dispositions_without_scan_release())
        .unwrap();
    disposition(&s, 23, "executed", None, &[23], false);
    disposition(&s, 50, "unresolved", Some("purged"), &[50], false);
    disposition(&s, 51, "executed", None, &[3], false);
    drop(s);
    assert!(matches!(d.drain(None).unwrap(), DrainOutcome::Drained(_)));
    let j = d.journal_sql();
    assert_eq!(status(&j, 23), st("executed/executed"), "the live row wins");
    let h = holds(&j);
    assert!(h.contains(&hold(
        50,
        50,
        "unresolved",
        "unresolved",
        Some("purged"),
        false
    )));
    assert!(
        h.contains(&hold(
            23,
            23,
            "unresolved",
            "unresolved",
            Some("cancelled"),
            false
        )),
        "a hold already drained keeps its row"
    );
    assert!(
        h.contains(&hold(51, 3, "executed", "executed", None, true)),
        "C1: the earlier drained hold read the note spent"
    );
    drop(j);
    let s = d.store();
    assert!(!tables(&s).contains(LIVE));
    let retired = fork_rows(&s, RETIRED);
    assert_eq!(retired.len(), 20, "the union");
    assert!(retired.contains(&(23, "executed".into(), None, 32)));
    pending(&s, 9, &[9], b"after the retire", true);
    drop(s);
    assert!(matches!(d.drain(None).unwrap(), DrainOutcome::Drained(_)));
    assert_eq!(pending_activities(&d.store()), Vec::<u8>::new());
    assert_eq!(d.drain(None).unwrap(), DrainOutcome::NotForkFormat);
}

/// DRAIN rev 4 test 6: a drained directory drains as not fork format and
/// no file changes. So does a fresh store, before any precondition, and a
/// directory with no store, which gains no lock file.
#[test]
fn a_second_drain_finds_nothing() {
    let d = mapping_fixture();
    d.drain(None).unwrap();
    let before = d.sha256s();
    assert_eq!(d.drain(None).unwrap(), DrainOutcome::NotForkFormat);
    assert_eq!(d.sha256s(), before);

    let mut fresh = Dir::new();
    fresh.open_persister();
    assert_eq!(fresh.drain(None).unwrap(), DrainOutcome::NotForkFormat);
    assert!(!fresh.net.join("shielded.sqlite.lock").exists());
    let s = Connection::open(fresh.store_path()).unwrap();
    s.execute_batch("CREATE TABLE shielded_pending_spends (wallet_id BLOB, nullifiers BLOB)")
        .unwrap();
    drop(s);
    let before = fresh.sha256s();
    assert_eq!(
        fresh.drain(None).unwrap(),
        DrainOutcome::NotForkFormat,
        "no snapshot is needed when there is nothing to drain"
    );
    assert_eq!(fresh.sha256s(), before);
}

/// DRAIN rev 4 test 7: an undecodable key or held list refuses the drain,
/// and nothing changes.
#[test]
fn a_corrupt_key_or_held_list_refuses_the_drain_and_keeps_the_store_closed() {
    for (case, wallet, held) in [
        ("key", vec![0xa1; 31], list(&[1])),
        ("held list", W.to_vec(), vec![7; 33]),
    ] {
        let mut d = Dir::new();
        let s = d.fork_store(Some(FORK_DISPOSITIONS_UNCHECKED));
        disposition(&s, 1, "executed", None, &[1], false);
        s.execute(
            "INSERT INTO shielded_spend_dispositions (wallet_id, account_index, st_hash, \
             activity_id, disposition, reason, at, nullifiers) \
             VALUES (?1, 0, ?2, ?2, 'executed', NULL, 1, ?3)",
            params![wallet, &n(2)[..], held],
        )
        .unwrap();
        drop(s);
        drop(d.journal());
        d.snapshot_and_open();
        let before = d.sha256s();
        let err = d.drain(None).unwrap_err();
        assert!(
            matches!(err, ShieldedDrainError::Corrupt { .. }),
            "{case}: {err:?}"
        );
        assert_eq!(d.sha256s(), before, "{case}");
        assert_eq!(
            d.journal()
                .shielded_drain_state(SHIELDED_STORE_FILE)
                .unwrap(),
            None
        );
    }
}

/// The precondition: without this directory's snapshot, or with another
/// directory's persister, the drain refuses and no file changes.
#[test]
fn the_drain_refuses_without_the_snapshot_and_changes_nothing() {
    let mut d = Dir::new();
    let s = d.fork_store(Some(FORK_DISPOSITIONS));
    disposition(&s, 1, "executed", None, &[1], false);
    drop(s);
    drop(d.journal());
    d.open_persister();
    let before = d.sha256s();
    assert_eq!(d.drain(None), Err(ShieldedDrainError::Precondition));
    assert_eq!(d.sha256s(), before);

    let mut other = Dir::new();
    other.open_persister();
    let journal = d.journal();
    assert_eq!(
        run(&d.net, other.opened(), &journal, None),
        Err(ShieldedDrainError::Precondition),
        "another directory's persister"
    );
    assert_eq!(d.sha256s(), before);
}

/// A real between-state store from the store upgrade: its fork fixture,
/// with this crate's fork shielded store (one executed disposition holding
/// note 1) in place before the first snapshot, R1's drop, and `before`
/// applied to the before-upgrade snapshot. With `again`, one more start's
/// prepare and reconcile ran, keeping a copy. No persister is open yet.
fn between(before: BeforeSnapshot, again: bool) -> Dir {
    let tmp = tempfile::Builder::new().prefix("dwsd-").tempdir().unwrap();
    let root = tmp.path().join("data");
    let net = root.join("regtest");
    private_dir(&root);
    between_state_with(&net, before, again, SystemTime::now(), |dir| {
        for name in ["", "-wal", "-shm", "-journal"] {
            let path = dir.join(format!("{SHIELDED_STORE_FILE}{name}"));
            if path.exists() {
                std::fs::remove_file(path)?;
            }
        }
        let conn = Connection::open(dir.join(SHIELDED_STORE_FILE)).unwrap();
        conn.execute_batch(FORK_PENDING).unwrap();
        conn.execute_batch(FORK_DISPOSITIONS).unwrap();
        disposition(&conn, 1, "executed", None, &[1], false);
        Ok(())
    })
    .unwrap();
    Dir {
        _tmp: tmp,
        root,
        net,
        persister: None,
    }
}

/// Q110 110-1, over the store upgrade's real between state: the drain
/// runs on the part-upgraded copy a part-done upgrade takes when it has no
/// verified before-upgrade snapshot (absent, damaged or another wallet's),
/// runs on a verified before-upgrade snapshot, and refuses, changing
/// nothing, with neither. Each runs after the real upstream open.
#[test]
fn the_drain_runs_on_this_upgrades_copy_and_refuses_without_one() {
    for before in [
        BeforeSnapshot::Absent,
        BeforeSnapshot::Unverified,
        BeforeSnapshot::Mismatch,
    ] {
        let mut d = between(before, true);
        assert!(
            !dw_store_upgrade::snapshot_present(&d.net).unwrap(),
            "{before:?}: no verified before-upgrade snapshot"
        );
        d.open_persister();
        assert!(
            matches!(d.drain(None), Ok(DrainOutcome::Drained(_))),
            "{before:?}: drained on the part-upgraded copy"
        );
        assert_eq!(
            d.journal().shielded_drained_holds(&W).unwrap(),
            vec![(0, n(1))],
            "{before:?}"
        );
    }

    for again in [false, true] {
        let mut d = between(BeforeSnapshot::Present, again);
        assert!(dw_store_upgrade::snapshot_present(&d.net).unwrap());
        d.open_persister();
        assert!(
            matches!(d.drain(None), Ok(DrainOutcome::Drained(_))),
            "a verified before-upgrade snapshot, again = {again}"
        );
    }

    let mut d = between(BeforeSnapshot::Absent, false);
    assert!(!upgrade_copy_present(&d.net, SHIELDED_STORE_FILE).unwrap());
    d.open_persister();
    drop(d.journal());
    let before = d.sha256s();
    assert_eq!(d.drain(None), Err(ShieldedDrainError::Precondition));
    assert_eq!(d.sha256s(), before, "neither copy: nothing changed");
}

#[test]
fn shielded_drain_retries_after_wallet_removal_retires_the_snapshot() {
    let mut d = between(BeforeSnapshot::Present, false);
    let snapshot = dw_store_upgrade::latest_snapshot(&d.net).unwrap().unwrap();
    d.open_persister();
    assert!(d.drain(Some(FailAfter::Read)).is_err());
    drop(d.persister.take());
    let adopters = Adopters::for_build(&d.root);
    let config = SqlitePersisterConfig::new(d.net.join("wallet.sqlite"))
        .with_load_policy(LoadPolicy::Strict)
        .with_auto_backup_dir(Some(d.net.join("backups").join("auto")));
    let prepared = dw_store_upgrade::prepare_with(&d.net, adopters, SystemTime::now()).unwrap();
    assert!(matches!(
        dw_store_upgrade::reconcile_fork_store(&config, &prepared.snapshot.unwrap(), adopters),
        Ok(dw_store_upgrade::Outcome::Retired { .. })
    ));

    let wallet = Connection::open(d.net.join("wallet.sqlite")).unwrap();
    let id: Vec<u8> = wallet
        .query_row("SELECT wallet_id FROM wallets LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert!(wallet.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE name = 'dw_fork_schema_history_retired')", [], |r| r.get::<_, bool>(0)).unwrap());
    let id = id.try_into().unwrap();
    dw_store_upgrade::retire_snapshot_for_wallet_removal(&d.net, &id).unwrap();
    let marker = snapshot.dir().join(dw_store_upgrade::NOT_THIS_WALLET);
    let removal_marker = std::fs::read(&marker).unwrap();
    std::fs::write(&marker, b"dw-store-upgrade not this wallet\nformat 1\n").unwrap();
    assert!(!upgrade_copy_present(&d.net, SHIELDED_STORE_FILE).unwrap());
    std::fs::write(&marker, &removal_marker).unwrap();

    // Changing a parseable metadata field alters the manifest digest while
    // keeping store files intact and must fail the UPGRADED-FROM check.
    let complete = snapshot.dir().join("SNAPSHOT-COMPLETE");
    let original_complete = std::fs::read_to_string(&complete).unwrap();
    let changed_complete = original_complete.replacen("build ", "build tampered-", 1);
    assert_ne!(changed_complete, original_complete);
    std::fs::write(&complete, &changed_complete).unwrap();
    assert!(!upgrade_copy_present(&d.net, SHIELDED_STORE_FILE).unwrap());
    std::fs::write(&complete, &original_complete).unwrap();

    let shielded_copy = snapshot.dir().join(SHIELDED_STORE_FILE);
    let original = std::fs::read(&shielded_copy).unwrap();
    let mut damaged = original.clone();
    damaged[100] ^= 1;
    std::fs::write(&shielded_copy, damaged).unwrap();
    assert!(!upgrade_copy_present(&d.net, SHIELDED_STORE_FILE).unwrap());
    std::fs::write(&shielded_copy, original).unwrap();

    // A same-phrase import recreates the original wallet ID.
    wallet
        .execute_batch(
            "CREATE TEMP TABLE reimported AS SELECT * FROM wallets; DELETE FROM wallets;",
        )
        .unwrap();
    assert!(upgrade_copy_present(&d.net, SHIELDED_STORE_FILE).unwrap());
    wallet
        .execute_batch("INSERT INTO wallets SELECT * FROM reimported;")
        .unwrap();
    drop(wallet);
    assert!(dw_store_upgrade::request_restore(&d.net, &snapshot).is_err());
    d.open_persister();
    assert!(matches!(d.drain(None), Ok(DrainOutcome::Drained(_))));
    assert_eq!(
        d.journal().shielded_drained_holds(&W).unwrap(),
        vec![(0, n(1))]
    );
}

#[test]
fn shielded_drain_propagates_upgrade_copy_io_error() {
    let mut d = between(BeforeSnapshot::Present, false);
    d.open_persister();
    drop(d.journal());
    let backups = d.net.join(dw_store_upgrade::UPGRADE_BACKUPS_DIR);
    std::fs::remove_dir_all(&backups).unwrap();
    std::fs::write(&backups, b"not a directory").unwrap();
    let err = d.drain(None).unwrap_err();
    assert!(
        matches!(err, ShieldedDrainError::Io(_)),
        "expected Io error when upgrade-backups check fails, got {err:?}"
    );
}

/// Step 0: while a store holds the owner lock the drain refuses.
#[test]
fn the_drain_refuses_while_a_store_holds_the_file() {
    let d = mapping_fixture();
    let lock = File::create(d.net.join("shielded.sqlite.lock")).unwrap();
    lock.try_lock().unwrap();
    assert_eq!(d.drain(None), Err(ShieldedDrainError::StoreInUse));
    drop(lock);
    assert!(matches!(d.drain(None).unwrap(), DrainOutcome::Drained(_)));
}

/// F5 after a lost journal: a live row over a retired row of the same key
/// holds the notes of both.
#[test]
fn a_lost_journal_with_both_tables_holds_the_notes_of_both() {
    let d = mapping_fixture();
    d.drain(None).unwrap();
    let s = d.store();
    s.execute_batch(FORK_DISPOSITIONS).unwrap();
    disposition(&s, 1, "executed", None, &[9], false);
    disposition(&s, 10, "executed", None, &[], false);
    drop(s);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(d.net.join(format!("{DISPATCH_DB_FILE}{suffix}")));
    }
    assert!(matches!(d.drain(None).unwrap(), DrainOutcome::Drained(_)));
    let held: BTreeSet<(u8, u8)> = holds(&d.journal_sql())
        .into_iter()
        .map(|(st, nf, ..)| (st, nf))
        .collect();
    for want in [(1, 1), (1, 2), (1, 9)] {
        assert!(held.contains(&want), "{want:?}");
    }
}

/// Q96 96-3: the drain carries the fork store's commitment tree, so
/// upstream's first open of the drained file bounds its heightless records
/// by that tree's size, and the drained hold stands in the journal.
#[test]
fn should_keep_the_fork_tree_as_the_heightless_bound_after_the_drain() {
    use platform_wallet::wallet::shielded::{FileBackedShieldedStore, ShieldedStore, SubwalletId};
    const T: u8 = 7;
    let mut d = Dir::new();
    {
        let mut upstream = FileBackedShieldedStore::open_path(d.store_path(), 100).unwrap();
        for leaf in 0..T {
            upstream.append_commitment(&[leaf + 1; 32], true).unwrap();
        }
    }
    // Back to the fork's shape: the fork wrote the same commitment tree
    // (its grovedb-commitment-tree source is the same) and none of
    // upstream's later tables or columns.
    let s = d.store();
    s.execute_batch(
        "DROP TABLE shielded_store_meta;
         DROP TABLE IF EXISTS shielded_private_payment_journal;
         DROP TABLE IF EXISTS shielded_private_payment_journal_wallets;
         ALTER TABLE shielded_pending_spends DROP COLUMN anchor_height;
         ALTER TABLE shielded_pending_spends DROP COLUMN private_contact_payment;
         ALTER TABLE shielded_pending_spends ADD COLUMN unsent INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE shielded_pending_spends ADD COLUMN armed_recording INTEGER NOT NULL DEFAULT 0;",
    )
    .unwrap();
    s.execute_batch(FORK_DISPOSITIONS).unwrap();
    disposition(&s, 1, "executed", None, &[1], false);
    pending(&s, 2, &[2], b"never sent", true);
    drop(s);
    drop(d.journal());
    d.snapshot_and_open();
    assert!(matches!(d.drain(None).unwrap(), DrainOutcome::Drained(_)));
    assert_eq!(
        d.journal().shielded_drained_holds(&W).unwrap(),
        vec![(0, n(1))]
    );
    let upstream = FileBackedShieldedStore::open_path(d.store_path(), 100).unwrap();
    assert_eq!(upstream.tree_size().unwrap(), u64::from(T));
    assert_eq!(
        upstream.legacy_anchor_tree_bound(),
        Some(u64::from(T)),
        "the carried tree bounds every heightless record"
    );
    let id = SubwalletId {
        wallet_id: W,
        account_index: 0,
    };
    assert!(
        upstream.pending_redrives(id).unwrap().is_empty(),
        "the never-sent record is not rehydrated"
    );
}
