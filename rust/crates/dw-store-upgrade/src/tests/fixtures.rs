//! The released fixtures through the whole upgrade.

use std::collections::BTreeSet;

use super::*;
use crate::precheck::{Precheck, precheck_with};
use crate::reconcile::{Outcome, reconcile_fork_store};
use crate::snapshot::{MARKER, STORE_FILES, take_snapshot};
use crate::sql::{has_table, integrity_ok, open_immutable, sha256_file};

fn fork_versions(path: &Path) -> BTreeSet<u32> {
    let conn = open_immutable(path).unwrap();
    conn.prepare("SELECT version FROM dw_fork_schema_history")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn upgrade(version: &str, want: &[u32]) {
    let f = fixture(version);
    let adopters = build();
    let versions = fork_versions(&f.wallet());
    assert_eq!(versions, want.iter().copied().collect::<BTreeSet<u32>>());
    let refinery_max: i64 = open_immutable(&f.wallet())
        .unwrap()
        .query_row(
            "SELECT MAX(version) FROM refinery_schema_history",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(refinery_max, crate::fork::FORK_REFINERY_MAX);

    let found = precheck_with(&f.net, adopters).unwrap();
    assert!(
        matches!(&found, Precheck::Fork { versions: v } if *v == versions),
        "{found:?}"
    );

    let snapshot = take_snapshot(&f.net, now()).unwrap();
    assert!(snapshot.dir().starts_with(f.net.join("upgrade-backups")));
    assert!(
        snapshot
            .dir()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("before-upgrade-20261125T")
    );
    let marker = fs::read_to_string(snapshot.dir().join(MARKER)).unwrap();
    assert!(marker.contains(&format!("build {}", env!("CARGO_PKG_VERSION"))));
    assert!(marker.contains("created 20261125T005000Z"));
    assert_eq!(
        snapshot.files().len() + snapshot.absent().len(),
        STORE_FILES.len()
    );
    for file in snapshot.files() {
        let copy = snapshot.dir().join(&file.name);
        assert_eq!(sha256_file(&copy).unwrap(), file.sha256, "{}", file.name);
        assert!(marker.contains(&format!(
            "file {} {} {}",
            file.name, file.bytes, file.sha256
        )));
        let conn = open_immutable(&copy).unwrap();
        assert!(integrity_ok(&conn).unwrap(), "{}", file.name);
        assert_eq!(
            tables(&copy),
            tables(&f.net.join(&file.name)),
            "{} copy differs from the live store",
            file.name
        );
    }
    assert!(
        !f.net
            .join("backups/auto")
            .join(snapshot.dir().file_name().unwrap())
            .exists()
    );

    let outcome = reconcile_fork_store(&config(&f.net), &snapshot, adopters).unwrap();
    assert_eq!(
        outcome,
        Outcome::Pending {
            versions: versions.clone(),
            notice: None,
        }
    );
    let conn = Connection::open(f.wallet()).unwrap();
    assert!(!crate::sql::has_column(&conn, "contacts", "external_account_reference").unwrap());
    let (_fresh_dir, fresh) = fresh_store();
    let fresh_conn = Connection::open(&fresh).unwrap();
    assert_eq!(columns(&conn, "contacts"), columns(&fresh_conn, "contacts"));
    drop(conn);

    let (persister, token) = open_and_load(&f.net);
    drop(persister);
    drop(token);

    assert!(matches!(
        precheck_with(&f.net, adopters).unwrap(),
        Precheck::Fork { .. }
    ));
    let again = take_snapshot(&f.net, later(60)).unwrap();
    assert_eq!(
        again.dir(),
        snapshot.dir(),
        "the in-flight upgrade reuses its snapshot"
    );
    let outcome = reconcile_fork_store(&config(&f.net), &snapshot, adopters).unwrap();
    assert_eq!(outcome, Outcome::Retired { versions });
    let conn = open_immutable(&f.wallet()).unwrap();
    assert!(!has_table(&conn, "dw_fork_schema_history").unwrap());
    assert!(has_table(&conn, "dw_fork_schema_history_retired").unwrap());
    drop(conn);
    assert!(matches!(
        precheck_with(&f.net, adopters).unwrap(),
        Precheck::NotForked
    ));
    drop(open_and_load(&f.net));
}

#[test]
fn v0_1_0_upgrades_and_retires() {
    upgrade("v0.1.0", &[1]);
}

#[test]
fn v0_2_0_upgrades_and_retires() {
    upgrade("v0.2.0", &[1]);
}

#[test]
fn v0_3_0_upgrades_and_retires() {
    upgrade("v0.3.0", &[1, 2, 3]);
}

#[test]
fn snapshot_is_never_under_upstream_auto_backups_and_prune_cannot_reach_it() {
    let f = fixture("v0.1.0");
    let snapshot = take_snapshot(&f.net, now()).unwrap();
    let auto = platform_wallet_storage::default_auto_backup_dir(&f.wallet());
    assert!(!snapshot.dir().starts_with(&auto));
    let before = tree(&f.net.join("upgrade-backups"));
    for dir in [
        auto.clone(),
        f.net.join("upgrade-backups"),
        snapshot.dir().to_path_buf(),
    ] {
        fs::create_dir_all(&dir).unwrap();
        let _ = platform_wallet_storage::prune_backups_in(
            &dir,
            platform_wallet_storage::RetentionPolicy {
                keep_last_n: Some(0),
                max_age: Some(Duration::ZERO),
            },
        );
    }
    assert_eq!(tree(&f.net.join("upgrade-backups")), before);
}

#[test]
fn incomplete_snapshot_dir_is_discarded_and_redone() {
    let f = fixture("v0.2.0");
    let stale = f
        .net
        .join("upgrade-backups/before-upgrade-20261124T000000Z");
    fs::create_dir_all(&stale).unwrap();
    fs::write(stale.join("wallet.sqlite"), b"partial").unwrap();
    let snapshot = take_snapshot(&f.net, now()).unwrap();
    assert!(!stale.exists());
    assert_ne!(snapshot.dir(), stale);
    assert!(crate::snapshot::snapshot_present(&f.net).unwrap());
}

#[test]
fn snapshot_is_redone_when_the_data_changed_without_an_upgrade() {
    let f = fixture("v0.1.0");
    let first = take_snapshot(&f.net, now()).unwrap();
    exec(&f.wallet(), "UPDATE core_utxos SET spent = 1 - spent");
    let second = take_snapshot(&f.net, later(1)).unwrap();
    assert_ne!(first.dir(), second.dir());
    assert!(
        first.dir().join(MARKER).exists(),
        "a completed snapshot is never deleted"
    );
}
