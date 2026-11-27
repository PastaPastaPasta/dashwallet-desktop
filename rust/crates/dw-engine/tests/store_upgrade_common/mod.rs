//! Shared by the store-upgrade open tests: copies of the released fixtures
//! (testdata/upgrade/*, never edited) in 0700 temp data roots, opened
//! through the real `Engine::open_network`.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dw_engine::store_upgrade::NO_SCREEN;
use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, SessionOptions,
};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use sha2::{Digest, Sha256};

pub struct NoEvents;

impl EventSink for NoEvents {
    fn emit(&self, _: EngineEvent) {}
}

pub fn local_opts() -> SessionOptions {
    SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        ..Default::default()
    }
}

pub fn engine(root: &Path) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.to_path_buf(),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(NoEvents),
    )
    .unwrap()
}

/// A data root holding the fixture's `regtest` network directory.
pub struct Fixture {
    _tmp: tempfile::TempDir,
    pub root: PathBuf,
    pub net: PathBuf,
}

impl Fixture {
    pub fn wallet(&self) -> PathBuf {
        self.net.join("wallet.sqlite")
    }
}

fn private_dir(path: &Path) {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut b = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path).unwrap();
}

fn copy_tree(src: &Path, dest: &Path) {
    private_dir(dest);
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dest.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), &to).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&to, fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
    }
}

pub fn fixture(version: &str) -> Fixture {
    let tmp = dw_testutil::private_tempdir();
    let root = tmp.path().join("data");
    private_dir(&root);
    let net = root.join("regtest");
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/upgrade")
        .join(version)
        .join("data/regtest");
    copy_tree(&src, &net);
    Fixture {
        _tmp: tmp,
        root,
        net,
    }
}

pub fn exec(path: &Path, sql: &str) {
    rusqlite::Connection::open(path)
        .unwrap()
        .execute_batch(sql)
        .unwrap();
}

/// `dir` itself (as ".") and every file and directory under it: relative
/// path → its mode and sha256, or its mode and "dir".
pub fn tree(dir: &Path) -> BTreeMap<String, String> {
    fn mode(meta: &fs::Metadata) -> String {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            format!("{:o}", meta.permissions().mode() & 0o7777)
        }
        #[cfg(not(unix))]
        {
            format!("ro={}", meta.permissions().readonly())
        }
    }
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let rel = (entry.path().strip_prefix(base).unwrap())
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            let meta = fs::symlink_metadata(entry.path()).unwrap();
            if meta.is_dir() {
                out.insert(rel, format!("{} dir", mode(&meta)));
                walk(base, &entry.path(), out);
            } else {
                // Windows denies even an EOF read on an exclusively locked file.
                let bytes = if meta.is_file() && meta.len() == 0 {
                    Vec::new()
                } else {
                    fs::read(entry.path()).unwrap()
                };
                out.insert(
                    rel,
                    format!("{} {}", mode(&meta), hex::encode(Sha256::digest(&bytes))),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    out.insert(
        ".".into(),
        format!("{} dir", mode(&fs::metadata(dir).unwrap())),
    );
    walk(dir, dir, &mut out);
    out
}

/// The paths whose mode or hash differs between two trees, or that one lacks.
pub fn tree_diff(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) -> Vec<String> {
    let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter(|k| a.get(*k) != b.get(*k))
        .cloned()
        .collect()
}

/// What a refused open may add to the network directory: the snapshot and
/// upstream's auto-backup directory. Everything that was there stays
/// byte-identical, mode included.
const MAY_ADD: [&str; 2] = ["upgrade-backups", "backups/auto"];

/// [`tree_diff`] less what a refused open may add.
pub fn refused_open_diff(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<String> {
    let added = |k: &str| {
        !before.contains_key(k)
            && MAY_ADD.iter().any(|d| {
                k == *d || k.starts_with(&format!("{d}/")) || d.starts_with(&format!("{k}/"))
            })
    };
    (tree_diff(before, after).into_iter())
        .filter(|k| !added(k))
        .collect()
}

/// Opens `f`'s network through a new engine on its data root and requires
/// it refused with every existing file and directory of the network
/// directory (the four stores, their logs, the vault, everything, and the
/// directory itself) byte-identical afterwards, modes included; only
/// [`MAY_ADD`] may be new. Returns the
/// refusal's `StoreUpgradeError` variant: from `storage_refused`, or from
/// the `internal` error a refusal without a screen maps to until
/// dw-store-upgrade has one (`every_refusal_has_a_screen`).
pub fn open_refused(f: &Fixture) -> String {
    refusal_cause(&open_refused_err(f))
}

/// [`open_refused`]'s error.
pub fn open_refused_err(f: &Fixture) -> EngineError {
    let before = tree(&f.net);
    let engine = engine(&f.root);
    let err = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .err()
        .expect("the open is refused");
    let changed = refused_open_diff(&before, &tree(&f.net));
    assert!(
        changed.is_empty(),
        "the refused open changed {changed:?} ({err})"
    );
    engine.block_on(engine.shutdown()).unwrap();
    err
}

/// The refusal's `StoreUpgradeError` variant name.
pub fn refusal_cause(err: &EngineError) -> String {
    match err {
        EngineError::StoreRefused(r) => {
            let text = r.screen.text("");
            assert!(!text.is_empty());
            r.cause.to_string()
        }
        EngineError::Internal(m) if m.starts_with(NO_SCREEN) => m[NO_SCREEN.len()..].to_string(),
        other => panic!("not a store refusal: {other:?}"),
    }
}

/// A retired contact-pool row, as a fork build rotating a contact writes it.
pub fn add_retired_row(path: &Path) {
    exec(
        path,
        "INSERT INTO core_address_pool_retired (wallet_id, account_type, account_index, account_xpub, \
           pool_type, address_index, script, used) \
         SELECT wallet_id, 'dashpay_external', 0, x'0488b21e', 2, 0, x'76a914' || randomblob(20) || x'88ac', 1 \
         FROM wallets LIMIT 1",
    );
}
