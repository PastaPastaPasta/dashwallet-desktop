//! Its own test binary: this process's probe is cached reading Broken, and
//! every wallet open is refused with nothing changed (Q78-A A4).

mod store_upgrade_common;

use dw_engine::{DashNetwork, EngineError};
use dw_store_upgrade::{Adopters, P9aStatus};
use store_upgrade_common::*;

#[test]
fn a_broken_p9a_refuses_every_open_unchanged() {
    let probe_root = dw_testutil::private_tempdir();
    Adopters::cache_with_p9a_for_test(
        probe_root.path(),
        P9aStatus::Broken {
            detail: "control FAILED: C1".into(),
        },
    );
    for version in ["v0.1.0", "v0.3.0"] {
        let f = fixture(version);
        assert_eq!(open_refused(&f), "P9aBroken", "{version}");
    }

    let fresh = dw_testutil::private_tempdir();
    let engine = engine(fresh.path());
    let err = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .err()
        .expect("refused");
    assert_eq!(refusal_cause(&err), "P9aBroken");
    assert!(matches!(err, EngineError::StoreRefused(_)), "{err:?}");
    assert!(
        !fresh.path().join("regtest/wallet.sqlite").exists(),
        "no store was created"
    );
    engine.block_on(engine.shutdown()).unwrap();
}
