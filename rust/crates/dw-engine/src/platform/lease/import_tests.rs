//! The session's open of a schema 10 dispatch journal: its shielded spends
//! are imported and read against the wallets the session knows (loaded,
//! unloaded, or listed in the app database); only those of a wallet it
//! knows by none of these are archived.

use std::path::Path;
use std::sync::Arc;

use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use rusqlite::{Connection, params};
use zeroize::Zeroizing;

use crate::{DashNetwork, Engine, EngineConfig, ImportOptions, NetworkSession, WalletId};

const PHRASE_A: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const PHRASE_B: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";

struct NullSink;
impl crate::EventSink for NullSink {
    fn emit(&self, _: crate::EngineEvent) {}
}

fn options() -> crate::SessionOptions {
    crate::SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        ..Default::default()
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    engine: Engine,
    session: Arc<NetworkSession>,
}

impl Fixture {
    fn new() -> Self {
        let dir = dw_testutil::private_tempdir();
        let engine = Engine::new(
            EngineConfig {
                data_root: dir.path().join("data"),
                worker_threads: Some(2),
                vault: VaultConfig {
                    kdf: KdfPolicy::Fixed(KdfParams::TEST),
                    os_store: Arc::new(MemoryOsStore::new()),
                    ..VaultConfig::default()
                },
            },
            Arc::new(NullSink),
        )
        .unwrap();
        let session = engine
            .block_on(engine.open_network(DashNetwork::Regtest, options()))
            .unwrap();
        engine
            .block_on(session.vault_op(|v| v.create(None)))
            .unwrap();
        Self {
            _dir: dir,
            engine,
            session,
        }
    }

    fn import(&self, phrase: &str) -> WalletId {
        self.engine
            .block_on(self.session.import_wallet(
                Zeroizing::new(phrase.as_bytes().to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions {
                    birth_height: Some(0),
                    core_compat: false,
                    ..Default::default()
                },
            ))
            .unwrap()
    }

    fn close(&self) -> std::path::PathBuf {
        let data_dir = self.session.data_dir().to_path_buf();
        self.engine
            .block_on(self.engine.close_network(DashNetwork::Regtest))
            .unwrap();
        data_dir
    }

    fn open(&mut self) {
        self.session = self
            .engine
            .block_on(self.engine.open_network(DashNetwork::Regtest, options()))
            .unwrap();
        assert!(self.session.leases.journal_ready());
    }
}

/// The closed session's journal as a schema 10 build leaves it, holding
/// the redrive rows `redrives` `(wallet, activity, transition)` and the
/// markers `markers` `(wallet, activity, anchor, nullifiers)`.
fn as_schema_10(
    data_dir: &Path,
    redrives: &[([u8; 32], [u8; 32], [u8; 32])],
    markers: &[([u8; 32], [u8; 32], [u8; 32], &[[u8; 32]])],
) {
    let conn = Connection::open(data_dir.join(dw_appdb::dispatch::DISPATCH_DB_FILE)).unwrap();
    conn.execute_batch(
        "DROP TABLE IF EXISTS shielded_artifact; DROP TABLE IF EXISTS shielded_artifact_note;
         DROP TABLE IF EXISTS shielded_artifact_published;
         UPDATE meta SET v = 10 WHERE k = 'schema';",
    )
    .unwrap();
    for (wallet, activity, transition) in redrives {
        conn.execute(
            "INSERT INTO shielded_redrive (wallet, activity, transition, at) VALUES (?1, ?2, ?3, 1)",
            params![&wallet[..], &activity[..], &transition[..]],
        )
        .unwrap();
    }
    for (wallet, activity, anchor, nullifiers) in markers {
        conn.execute(
            "INSERT INTO shielded_unrecorded (wallet, activity, anchor, at) VALUES (?1, ?2, ?3, 1)",
            params![&wallet[..], &activity[..], &anchor[..]],
        )
        .unwrap();
        for nullifier in *nullifiers {
            conn.execute(
                "INSERT INTO shielded_unrecorded_nullifier (wallet, activity, nullifier) \
                 VALUES (?1, ?2, ?3)",
                params![&wallet[..], &activity[..], &nullifier[..]],
            )
            .unwrap();
        }
    }
}

/// The open session's shielded artifacts, `(wallet, activity, origin,
/// st_hash, status, status_reason)`, read from its journal file.
type ArtifactRow = (
    [u8; 32],
    [u8; 32],
    String,
    Option<Vec<u8>>,
    String,
    Option<String>,
);

fn artifacts(session: &NetworkSession) -> Vec<ArtifactRow> {
    let conn = Connection::open(
        session
            .data_dir()
            .join(dw_appdb::dispatch::DISPATCH_DB_FILE),
    )
    .unwrap();
    let schema: i64 = conn
        .query_row("SELECT v FROM meta WHERE k = 'schema'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(schema, 11);
    let mut stmt = conn
        .prepare(
            "SELECT wallet, activity, origin, st_hash, status, status_reason \
             FROM shielded_artifact ORDER BY activity",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok((
            r.get::<_, Vec<u8>>(0)?.try_into().unwrap(),
            r.get::<_, Vec<u8>>(1)?.try_into().unwrap(),
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn unresolved(wallet: WalletId, activity: [u8; 32], transition: [u8; 32]) -> ArtifactRow {
    (
        wallet.0,
        activity,
        "imported_redrive".into(),
        Some(transition.to_vec()),
        "unresolved".into(),
        None,
    )
}

/// The session opens over a schema 10 journal: a loaded wallet's redrive
/// row reads unresolved and its marker unresolved (unrecorded), with no
/// `st_hash`; a wallet the session does not know has its row archived.
#[test]
fn a_v10_journal_opens_with_its_imported_rows_classified() {
    let mut f = Fixture::new();
    let a = f.import(PHRASE_A);
    let gone = [0x77; 32];
    let data_dir = f.close();
    as_schema_10(
        &data_dir,
        &[(a.0, [1; 32], [10; 32]), (gone, [2; 32], [20; 32])],
        &[(a.0, [3; 32], [30; 32], &[[31; 32]])],
    );
    f.open();
    assert_eq!(
        artifacts(&f.session),
        [
            unresolved(a, [1; 32], [10; 32]),
            (
                gone,
                [2; 32],
                "imported_redrive".into(),
                Some([20; 32].to_vec()),
                "archived".into(),
                None
            ),
            (
                a.0,
                [3; 32],
                "imported_marker".into(),
                None,
                "unresolved".into(),
                Some("unrecorded".into())
            ),
        ]
    );
    // The 0.1 marker still refuses the close.
    assert!(matches!(
        f.engine
            .block_on(f.engine.close_network(DashNetwork::Regtest)),
        Err(crate::EngineError::ShieldedSpendUnresolved)
    ));
}

/// A wallet off the load-on-startup list is in the session's unloaded
/// set: its imported rows stay unresolved, even with no app database row
/// naming it.
#[test]
fn an_unloaded_wallets_imported_rows_stay_unresolved() {
    let mut f = Fixture::new();
    f.import(PHRASE_A);
    let b = f.import(PHRASE_B);
    f.engine
        .block_on(f.session.set_load_on_startup(b, false))
        .unwrap();
    let data_dir = f.close();
    Connection::open(data_dir.join(dw_appdb::APP_DB_FILE))
        .unwrap()
        .execute("DELETE FROM wallets WHERE wallet_id = ?1", [b.to_string()])
        .unwrap();
    as_schema_10(&data_dir, &[(b.0, [1; 32], [10; 32])], &[]);
    f.open();
    assert!(f.session.live().unwrap().store.is_unloaded(&b.0));
    assert_eq!(artifacts(&f.session), [unresolved(b, [1; 32], [10; 32])]);
}

/// A wallet neither loaded nor unloaded but listed in the app database
/// keeps its imported rows unresolved.
#[test]
fn a_wallet_listed_only_in_the_app_db_keeps_its_rows_unresolved() {
    let mut f = Fixture::new();
    f.import(PHRASE_A);
    let listed = WalletId([0x5a; 32]);
    let data_dir = f.close();
    Connection::open(data_dir.join(dw_appdb::APP_DB_FILE))
        .unwrap()
        .execute(
            "INSERT INTO wallets (wallet_id, name, created_at) VALUES (?1, 'listed', 1)",
            [listed.to_string()],
        )
        .unwrap();
    as_schema_10(&data_dir, &[(listed.0, [1; 32], [10; 32])], &[]);
    f.open();
    let live = f.session.live().unwrap();
    assert!(!live.store.is_unloaded(&listed.0));
    assert!(!live.manager.list_wallet_ids_blocking().contains(&listed.0));
    assert_eq!(
        artifacts(&f.session),
        [unresolved(listed, [1; 32], [10; 32])]
    );
}
