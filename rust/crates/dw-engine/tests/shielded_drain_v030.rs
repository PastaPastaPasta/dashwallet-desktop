//! DRAIN rev 4 test 9 (F8): the released v0.3.0 engine, started on a data
//! directory this build has upgraded and drained, refuses it and writes
//! nothing to its four stores. A 0.3 build reads a newer dispatch journal
//! without refusing it; the refusal is `wallet.sqlite`'s schema version.
//!
//! Ignored by default: it runs v0.3.0's `dwcli` (tag `v0.3.0`, 9262dbb0cb),
//! built separately and named by `DW_V030_DWCLI`.

mod store_upgrade_common;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use dw_engine::DashNetwork;
use dw_shielded_drain::test_support::{ForkRow, fork_store};
use store_upgrade_common::*;

/// "Fixture main" of the v0.1.0 fixture.
const MAIN: &str = "0ff35f248c69653efce8336ce14b78e29cd3bbaf4162c42246259850af2c2add";
const STORES: [&str; 4] = [
    "wallet.sqlite",
    "app.sqlite",
    "dispatch.sqlite",
    "shielded.sqlite",
];
const SIDECARS: [&str; 3] = ["-wal", "-journal", "-shm"];

/// Each store's sha256, and which of the stores' sidecars exist.
fn stores(net: &Path) -> (BTreeMap<String, String>, Vec<String>) {
    let all = tree(net);
    let hashes = STORES
        .iter()
        .map(|s| ((*s).to_owned(), all.get(*s).cloned().expect(s)))
        .collect();
    let sidecars = STORES
        .iter()
        .flat_map(|s| SIDECARS.iter().map(move |x| format!("{s}{x}")))
        .filter(|f| all.contains_key(f))
        .collect();
    (hashes, sidecars)
}

#[test]
#[ignore = "runs the v0.3.0 dwcli named by DW_V030_DWCLI"]
fn a_0_3_0_engine_writes_nothing_to_an_upgraded_directory() {
    let dwcli =
        std::env::var_os("DW_V030_DWCLI").expect("DW_V030_DWCLI names v0.3.0's dwcli (tag v0.3.0)");
    let f = fixture("v0.1.0");
    let shielded = f.net.join("shielded.sqlite");
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(f.net.join(format!("shielded.sqlite{suffix}")));
    }
    let mut wallet = [0; 32];
    hex::decode_to_slice(MAIN, &mut wallet).unwrap();
    let nf = |n: u8| [n; 32];
    fork_store(
        &shielded,
        &wallet,
        0,
        &[
            ForkRow::Disposition {
                st: 1,
                kind: "executed",
                reason: None,
                nullifiers: vec![nf(101)],
            },
            ForkRow::Disposition {
                st: 2,
                kind: "unresolved",
                reason: Some("purged"),
                nullifiers: vec![nf(102)],
            },
        ],
    )
    .unwrap();

    let engine = engine(&f.root);
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .expect("this build upgrades and drains the fixture");
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);
    let live_fork_rows: i64 = rusqlite::Connection::open(&shielded)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'shielded_spend_dispositions'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(live_fork_rows, 0, "the drain retired the fork rows");

    let (before, sidecars_before) = stores(&f.net);
    let out = Command::new(&dwcli)
        .arg("--datadir")
        .arg(&f.root)
        .args(["--network", "regtest"])
        .args(["--dapi", "http://127.0.0.1:1"])
        .args(["--peer", "127.0.0.1:1"])
        .arg("list")
        .output()
        .expect("v0.3.0's dwcli runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    eprintln!("v0.3.0 dwcli: {}\n{stderr}", out.status);
    assert!(
        !out.status.success(),
        "v0.3.0 refuses the upgraded directory"
    );
    let (after, sidecars_after) = stores(&f.net);
    for (store, hash) in &before {
        eprintln!("{store}: {hash} → {}", after[store]);
    }
    assert_eq!(after, before, "the four stores are byte-identical");
    assert_eq!(sidecars_after, sidecars_before, "no sidecar left behind");
}
