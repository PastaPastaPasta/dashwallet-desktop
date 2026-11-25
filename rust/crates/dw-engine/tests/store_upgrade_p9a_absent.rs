//! Its own test binary: this process's probe is cached reading Absent, as
//! a build without P9a does, and the real open path meets it (Q78-A).

mod store_upgrade_common;

use dw_engine::EngineError;
use dw_store_upgrade::{Adopters, P9aStatus};
use store_upgrade_common::*;

#[test]
fn a_rotated_contact_fork_file_is_refused_unchanged_without_p9a() {
    let probe_root = dw_testutil::private_tempdir();
    Adopters::cache_with_p9a_for_test(probe_root.path(), P9aStatus::Absent);

    let f = fixture("v0.1.0");
    add_retired_row(&f.wallet());
    let err = open_refused_err(&f);
    assert_eq!(refusal_cause(&err), "RetiredPoolWithoutAdopter");
    let r = match err {
        EngineError::StoreRefused(r) => r,
        other => panic!("{other:?}"),
    };
    assert_eq!(r.folder, f.net);
    assert_eq!(r.screen.text_key, "storeUpgrade.cantOpen.textWithoutBackup");

    let empty = fixture("v0.1.0");
    let engine = engine(&empty.root);
    engine
        .block_on(engine.open_network(dw_engine::DashNetwork::Regtest, local_opts()))
        .expect("an empty retired table opens without P9a");
    engine.block_on(engine.shutdown()).unwrap();
}
