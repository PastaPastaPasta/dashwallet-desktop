//! The re-review of ffbc69cf: a snapshot whose marker was lost after R1's
//! drop (M-A) and snapshots of an upgraded file (M-B).

use super::review_fixes_3::{assert_missing, between};
use super::*;
use crate::snapshot::{ABSENT_POLICY_FOR_TEST, AbsentSnapshotPolicy, MARKER, MissingSnapshot};

#[test]
fn a_lost_marker_after_the_drop_is_unverified_and_the_copy_is_kept() {
    for policy in [
        AbsentSnapshotPolicy::Refuse,
        AbsentSnapshotPolicy::ProceedUnrestorable,
    ] {
        let (f, s) = between();
        fs::remove_file(s.dir().join(MARKER)).unwrap();
        ABSENT_POLICY_FOR_TEST.with(|p| p.set(Some(policy)));
        assert_missing(&f, MissingSnapshot::Unverified);
        ABSENT_POLICY_FOR_TEST.with(|p| p.set(None));
        assert!(s.dir().join("wallet.sqlite").is_file(), "{policy:?}");
    }
}
