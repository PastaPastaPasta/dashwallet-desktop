//! R2 calls through the FFI (docs/contracts/m2-engine.md §2.6–2.8): type
//! mapping and error codes on dashd-made vectors. The behaviour is tested in
//! dw-engine (tests/r2_compat_offline.rs) and against dashd in the regtest
//! `restore` suite.

use std::path::PathBuf;
use std::sync::Arc;

use crate::{
    CoreExportFormat, DashNetwork, Engine, EngineEvent, EngineObserver, ImportOptions, KeyMaterial,
    NetworkSession, PsbtSignability, PsbtStatus, SessionOptions, VaultCredential, WalletFileKind,
};

struct Null;
impl EngineObserver for Null {
    fn on_event(&self, _: EngineEvent) {}
}

fn testdata(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

struct Fixture {
    dir: tempfile::TempDir,
    engine: Arc<Engine>,
    session: Arc<NetworkSession>,
    rt: tokio::runtime::Runtime,
}

/// A regtest session with an unencrypted vault on an in-memory OS store.
fn fixture() -> Fixture {
    let dir = dw_testutil::private_tempdir();
    // The FFI constructor uses the OS keyring; tests use an in-memory store.
    let engine = Arc::new(Engine {
        inner: dw_engine::Engine::new(
            dw_engine::EngineConfig {
                data_root: dir.path().to_path_buf(),
                worker_threads: Some(2),
                vault: dw_vault::VaultConfig {
                    kdf: dw_vault::KdfPolicy::Fixed(dw_vault::KdfParams::TEST),
                    os_store: Arc::new(dw_vault::MemoryOsStore::new()),
                    ..dw_vault::VaultConfig::default()
                },
            },
            Arc::new(crate::api::engine::ObserverSink(Arc::new(Null))),
        )
        .unwrap(),
    });
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let session = rt
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
            },
        ))
        .unwrap();
    rt.block_on(session.vault().create(None)).unwrap();
    Fixture {
        dir,
        engine,
        session,
        rt,
    }
}

#[test]
fn test_qt_106_107_inspect_and_import_through_ffi() {
    let f = fixture();
    let kind =
        f.rt.block_on(
            f.engine
                .inspect_wallet_file(testdata("compat/walletdat/desc_encrypted.dat")),
        )
        .unwrap();
    assert_eq!(
        kind,
        WalletFileKind::WalletDatSqlite {
            encrypted: true,
            has_mnemonic: true
        }
    );
    let err =
        f.rt.block_on(f.session.import_wallet_dat(
            testdata("compat/walletdat/desc_encrypted.dat"),
            None,
            ImportOptions::default(),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "compat.passphrase_required");
    let err =
        f.rt.block_on(f.session.import_wallet_dat(
            testdata("compat/walletdat/bdb_legacy_head.dat"),
            None,
            ImportOptions::default(),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "not_implemented");
    assert!(err.to_string().ends_with("import_wallet_dat.bdb"), "{err}");

    let report =
        f.rt.block_on(f.session.import_dump_wallet(
            testdata("dumpwallet/dump_hd_basic.txt"),
            ImportOptions::default(),
        ))
        .unwrap();
    assert_eq!(report.keys_not_imported, 2);
    assert!(report.core_compat_seed);
    let err = f
        .rt
        .block_on(f.session.import_key_material(
            KeyMaterial::Xprv {
                xprv: b"xprv9s21ZrQH143K3QTDL4LXw2F7HEK3wJUD2nW2nRk4stbPy6cq3jPPqjiChkVvvNKmPGJxWUtg6LnF5kejMRNNU3TGtRBeJgk33yuGBxrMPHi".to_vec(),
            },
            ImportOptions::default(),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "compat.network_mismatch");

    let compat =
        f.rt.block_on(
            f.session
                .core_mnemonic_compatibility(report.wallet_id.clone()),
        )
        .unwrap();
    assert!(compat.core_compatible);
    let grant =
        f.rt.block_on(f.session.vault().authorize(
            crate::GrantPurpose::RevealSecret,
            Some(report.wallet_id.clone()),
            VaultCredential::Unencrypted,
        ))
        .unwrap();
    let dest = f.dir.path().join("out.txt").to_string_lossy().into_owned();
    let exported =
        f.rt.block_on(f.session.export_for_core(
            report.wallet_id.clone(),
            CoreExportFormat::DumpWallet,
            dest.clone(),
            grant.id,
        ))
        .unwrap();
    assert_eq!(exported.path, dest);
    let err =
        f.rt.block_on(f.session.export_for_core(
            report.wallet_id,
            CoreExportFormat::DumpWallet,
            dest,
            "spent".into(),
        ))
        .unwrap_err();
    assert_eq!(err.code(), "compat.destination_unwritable");
}

#[test]
fn test_qt_110_backup_codes_through_ffi() {
    let f = fixture();
    let id =
        f.rt.block_on(f.session.import_wallet(
            b"legal winner thank year wave sausage worth useful legal winner thank yellow".to_vec(),
            Vec::new(),
            ImportOptions::default(),
        ))
        .unwrap();
    let dest = f
        .dir
        .path()
        .join("w.dwbackup")
        .to_string_lossy()
        .into_owned();
    let err =
        f.rt.block_on(f.session.backup_wallet(id.clone(), dest.clone(), None))
            .unwrap_err();
    assert_eq!(err.code(), "backup.passphrase_required");
    let info =
        f.rt.block_on(
            f.session
                .backup_wallet(id.clone(), dest.clone(), Some(b"bk".to_vec())),
        )
        .unwrap();
    assert_eq!(info.wallet_id, id);
    let err =
        f.rt.block_on(f.session.restore_backup(dest, Some(b"bk".to_vec())))
            .unwrap_err();
    assert_eq!(err.code(), "backup.already_exists");
    let err = f.rt.block_on(f.session.set_backup_policy(11)).unwrap_err();
    assert_eq!(err.code(), "invalid_argument");
}

#[test]
fn test_qt_078_079_psbt_through_ffi() {
    let f = fixture();
    let text = std::fs::read(testdata("compat/psbt/unsigned.b64")).unwrap();
    let psbt = crate::parse_psbt(text.clone()).unwrap();
    assert_eq!(
        psbt.to_base64().unwrap(),
        String::from_utf8(text).unwrap().trim()
    );
    assert_eq!(
        crate::parse_psbt(psbt.to_bytes().unwrap())
            .unwrap()
            .unsigned_txid()
            .unwrap(),
        psbt.unsigned_txid().unwrap()
    );
    let a =
        f.rt.block_on(f.session.analyze_psbt(None, psbt.clone()))
            .unwrap();
    assert_eq!(a.status, PsbtStatus::NeedsSignatures);
    assert_eq!(a.signability, PsbtSignability::NoWallet);
    assert_eq!(a.unsigned_inputs, 2);
    assert_eq!(
        a.total,
        Some(a.outputs.iter().map(|o| o.amount).sum::<u64>() + a.fee.unwrap())
    );
    let err = f.rt.block_on(f.session.broadcast_psbt(psbt)).unwrap_err();
    assert_eq!(err.code(), "psbt.not_complete");
    let signed =
        crate::parse_psbt(std::fs::read(testdata("compat/psbt/signed.b64")).unwrap()).unwrap();
    // Complete, but SPV is not running: nothing was sent.
    let err = f.rt.block_on(f.session.broadcast_psbt(signed)).unwrap_err();
    assert_eq!(err.code(), "psbt.no_peers");
}
