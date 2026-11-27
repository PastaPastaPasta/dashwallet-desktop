//! [`crate::test_support::between_state`] builds each snapshot state it
//! offers on a real between-state store.

use super::*;
use crate::snapshot::{NOT_THIS_WALLET, SnapshotState, snapshot_state, upgrade_copy_present};
use crate::test_support::{BeforeSnapshot, between_state, between_state_with};

#[test]
fn between_state_builds_each_snapshot_state() {
    for before in [
        BeforeSnapshot::Present,
        BeforeSnapshot::Absent,
        BeforeSnapshot::Unverified,
        BeforeSnapshot::Mismatch,
    ] {
        let tmp = private_tempdir();
        let root = tmp.path().join("data");
        crate::fsx::create_private_dir(&root).unwrap();
        let net = root.join("regtest");
        build();
        let b = between_state(&net, before, true, now()).unwrap();
        let wallet = crate::sql::open_immutable(&net.join("wallet.sqlite")).unwrap();
        assert!(
            !crate::sql::has_column(&wallet, "contacts", "external_account_reference").unwrap(),
            "{before:?}: R1 committed"
        );
        let refinery: i64 = wallet
            .query_row(
                "SELECT MAX(version) FROM refinery_schema_history",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(refinery <= crate::fork::FORK_REFINERY_MAX, "{before:?}");
        let verified = matches!(snapshot_state(&net).unwrap(), SnapshotState::Verified(_));
        assert_eq!(verified, before == BeforeSnapshot::Present, "{before:?}");
        let kept = b.kept.unwrap();
        assert_eq!(
            kept.dir() == b.snapshot.dir(),
            before == BeforeSnapshot::Present,
            "{before:?}"
        );
        assert!(
            upgrade_copy_present(&net, "shielded.sqlite").unwrap(),
            "{before:?}"
        );
        assert_eq!(
            b.snapshot.dir().join(NOT_THIS_WALLET).is_file(),
            before == BeforeSnapshot::Mismatch,
            "{before:?}"
        );
    }
}

/// `adjust` puts the caller's store in place before the first prepare, and
/// both copies hold it: every table as written (SQLite's online backup
/// rewrites the header's change counter, so the file bytes differ there).
#[test]
fn between_state_with_copies_the_adjusted_store() {
    let tmp = private_tempdir();
    let root = tmp.path().join("data");
    crate::fsx::create_private_dir(&root).unwrap();
    let net = root.join("regtest");
    let own = tmp.path().join("own.sqlite");
    let conn = Connection::open(&own).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode = DELETE; CREATE TABLE own (a TEXT); \
         INSERT INTO own VALUES ('the caller''s shielded store')",
    )
    .unwrap();
    drop(conn);
    let written = tables(&own);
    build();
    let b = between_state_with(&net, BeforeSnapshot::Unverified, true, now(), |dir| {
        fs::copy(&own, dir.join("shielded.sqlite")).map(drop)
    })
    .unwrap();
    assert_eq!(tables(&net.join("shielded.sqlite")), written, "live");
    let part = b.kept.unwrap();
    for dir in [b.snapshot.dir(), part.dir()] {
        assert_eq!(tables(&dir.join("shielded.sqlite")), written, "{dir:?}");
    }
    let refused = between_state_with(
        &root.join("other"),
        BeforeSnapshot::Present,
        false,
        now(),
        |_| Err(std::io::Error::other("adjust failed")),
    )
    .unwrap_err();
    assert!(
        matches!(refused, crate::StoreUpgradeError::Io { .. }),
        "{refused:?}"
    );
}
