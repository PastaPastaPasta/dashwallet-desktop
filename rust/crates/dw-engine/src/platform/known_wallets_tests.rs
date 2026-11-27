//! The open classifies the 0.1 journal's imported shielded spends against
//! every wallet it knows: one of a wallet registered in wallet.sqlite stays
//! held (`unresolved`) even when the manager did not load that wallet and
//! no startup list or app.sqlite name lists it.

use dw_appdb::dispatch::DISPATCH_DB_FILE;

use super::close_unload_gate_tests::{ACTIVITY, engine_at, launch, open};

#[test]
fn an_import_of_a_registered_wallet_the_manager_did_not_load_stays_held() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = launch(&engine);
    let data_dir = session.data_dir().to_path_buf();
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();

    let app = rusqlite::Connection::open(data_dir.join("app.sqlite")).unwrap();
    app.execute("DELETE FROM settings_kv WHERE key = 'load_on_startup'", [])
        .unwrap();
    app.execute(
        "DELETE FROM wallets WHERE wallet_id = ?1",
        [wallet.to_string()],
    )
    .unwrap();
    drop(app);
    let journal = rusqlite::Connection::open(data_dir.join(DISPATCH_DB_FILE)).unwrap();
    journal
        .execute(
            "INSERT INTO shielded_artifact (wallet, activity, origin, st_hash, status, at) \
             VALUES (?1, ?2, 'imported_redrive', ?3, 'unclassified', 1)",
            rusqlite::params![wallet.0.to_vec(), ACTIVITY.to_vec(), [0x66u8; 32].to_vec()],
        )
        .unwrap();
    drop(journal);

    crate::session::KNOWN_WITHOUT_LOADED_IN
        .lock()
        .unwrap()
        .push(data_dir.clone());
    let session = open(&engine).unwrap();
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();

    let journal = rusqlite::Connection::open(data_dir.join(DISPATCH_DB_FILE)).unwrap();
    let status: String = journal
        .query_row(
            "SELECT status FROM shielded_artifact WHERE wallet = ?1 AND activity = ?2",
            rusqlite::params![wallet.0.to_vec(), ACTIVITY.to_vec()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "unresolved");
}
