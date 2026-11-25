//! The P9a refusal net (Q78-A, R1 amendment): rows in the retired contact
//! pool are refused unless the probe reads Present; an empty table opens.

use super::*;
use crate::adopters::P9aStatus;
use crate::error::StoreUpgradeError;
use crate::precheck::{Precheck, precheck_with};
use crate::reconcile::{Outcome, reconcile_fork_store};
use crate::snapshot::take_snapshot;

#[test]
fn probe_reads_absent_at_d902b51() {
    assert_eq!(
        build().p9a_status(),
        &P9aStatus::Absent,
        "{:#?}",
        build().observations()
    );
    assert!(
        build()
            .observations()
            .iter()
            .any(|o| o.starts_with("control ok: C1"))
    );
    assert!(
        build()
            .observations()
            .iter()
            .all(|o| !o.starts_with("control FAILED"))
    );
}

#[test]
#[ignore = "A5: needs a platform pin with P9a (integration-3 SHA)"]
fn probe_reads_present_with_p9a() {
    assert_eq!(
        build().p9a_status(),
        &P9aStatus::Present,
        "{:#?}",
        build().observations()
    );
}

#[test]
fn rotated_contact_fork_file_is_refused_without_p9a() {
    let f = fixture("v0.1.0");
    add_retired_row(&f.wallet());
    let adopters = build();
    assert_ne!(adopters.p9a_status(), &P9aStatus::Present);
    let before = tree(&f.root);
    let found = precheck_with(&f.net, adopters).unwrap();
    let Precheck::Refuse { reason } = found else {
        panic!("expected a refusal, got {found:?}");
    };
    assert!(
        matches!(
            reason,
            StoreUpgradeError::RetiredPoolWithoutAdopter { rows: 1 }
        ),
        "{reason:?}"
    );
    assert!(reason.file_unchanged() && reason.is_refusal());
    assert_eq!(
        tree(&f.root),
        before,
        "the refusal changed {}",
        f.root.display()
    );
}

#[test]
fn rotated_contact_refusal_comes_before_the_drop_and_any_migration() {
    let f = fixture("v0.1.0");
    add_retired_row(&f.wallet());
    let snapshot = take_snapshot(&f.net, now()).unwrap();
    let before = tree(&f.net);
    let err = reconcile_fork_store(&f.wallet(), &snapshot, &build().with_p9a(P9aStatus::Absent))
        .unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::RetiredPoolWithoutAdopter { .. }),
        "{err:?}"
    );
    assert_eq!(tree(&f.net), before);
    let conn = crate::sql::open_immutable(&f.wallet()).unwrap();
    assert!(crate::sql::has_column(&conn, "contacts", "external_account_reference").unwrap());
}

#[test]
#[ignore = "needs a platform pin with P9a (integration-3 SHA): the probe must read Present"]
fn rotated_contact_fork_file_counts_correctly_with_p9a() {
    assert_eq!(
        build().p9a_status(),
        &P9aStatus::Present,
        "{:#?}",
        build().observations()
    );
    let f = fixture("v0.1.0");
    add_retired_row(&f.wallet());
    assert!(matches!(
        precheck_with(&f.net, build()).unwrap(),
        Precheck::Fork { .. }
    ));
    let snapshot = take_snapshot(&f.net, now()).unwrap();
    assert!(matches!(
        reconcile_fork_store(&f.wallet(), &snapshot, build()).unwrap(),
        Outcome::Pending { .. }
    ));
    drop(open_and_load(&f.net));
    assert!(matches!(
        reconcile_fork_store(&f.wallet(), &snapshot, build()).unwrap(),
        Outcome::Retired { .. }
    ));
}

fn empty_retired_table_opens(adopters: &Adopters, open: bool) {
    let f = fixture("v0.1.0");
    let conn = crate::sql::open_immutable(&f.wallet()).unwrap();
    assert_eq!(
        crate::sql::count_rows(&conn, "core_address_pool_retired").unwrap(),
        0
    );
    drop(conn);
    assert!(matches!(
        precheck_with(&f.net, adopters).unwrap(),
        Precheck::Fork { .. }
    ));
    let snapshot = take_snapshot(&f.net, now()).unwrap();
    assert!(matches!(
        reconcile_fork_store(&f.wallet(), &snapshot, adopters).unwrap(),
        Outcome::Pending { .. }
    ));
    if open {
        drop(open_and_load(&f.net));
        assert!(matches!(
            reconcile_fork_store(&f.wallet(), &snapshot, adopters).unwrap(),
            Outcome::Retired { .. }
        ));
    }
}

#[test]
fn empty_retired_table_opens_without_p9a() {
    empty_retired_table_opens(&build().with_p9a(P9aStatus::Absent), true);
}

#[test]
fn empty_retired_table_opens_with_p9a() {
    empty_retired_table_opens(&build().with_p9a(P9aStatus::Present), false);
}

#[test]
fn broken_probe_refuses_every_open_unchanged() {
    let adopters = build().with_p9a(P9aStatus::Broken {
        detail: "test".into(),
    });
    for version in ["v0.1.0", "v0.3.0"] {
        let f = fixture(version);
        let before = tree(&f.root);
        let found = precheck_with(&f.net, &adopters).unwrap();
        assert!(
            matches!(
                found,
                Precheck::Refuse {
                    reason: StoreUpgradeError::P9aBroken { .. }
                }
            ),
            "{found:?}"
        );
        assert_eq!(tree(&f.root), before);
    }
    let empty = private_tempdir();
    assert!(matches!(
        precheck_with(empty.path(), &adopters).unwrap(),
        Precheck::Refuse {
            reason: StoreUpgradeError::P9aBroken { .. }
        }
    ));
}

#[test]
#[ignore = "A7: run in the scratch workspace pinned at fe5b9da942 (retired table without the fixes)"]
fn rotated_contact_fork_file_is_refused_at_table_only_p9a() {
    assert!(
        matches!(build().p9a_status(), P9aStatus::Broken { .. }),
        "{:?}\n{:#?}",
        build().p9a_status(),
        build().observations()
    );
    let f = fixture("v0.1.0");
    add_retired_row(&f.wallet());
    let before = tree(&f.root);
    assert!(matches!(
        precheck_with(&f.net, build()).unwrap(),
        Precheck::Refuse {
            reason: StoreUpgradeError::P9aBroken { .. }
        }
    ));
    assert_eq!(tree(&f.root), before);
}
