//! [`crate::test_support::between_state`] builds each snapshot state it
//! offers on a real between-state store.

use super::*;
use crate::snapshot::{NOT_THIS_WALLET, SnapshotState, snapshot_state, upgrade_copy_present};
use crate::test_support::{BeforeSnapshot, between_state};

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
