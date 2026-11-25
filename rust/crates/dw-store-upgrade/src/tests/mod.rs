//! Tests over copies of the released fixtures (testdata/upgrade/*, never
//! edited), each in its own 0700 temp directory.

mod copy_table;
mod crash;
mod fixtures;
mod meta;
mod p9a;
mod refusals;
mod restore;
mod review_fixes;
mod review_fixes_2;
mod review_fixes_3;
mod startup_order;
mod v3;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use platform_wallet_storage::{LoadPolicy, SqlitePersister, SqlitePersisterConfig};
use rusqlite::Connection;

use crate::adopters::Adopters;
use crate::token::PersisterOpened;

pub(crate) fn repo_root() -> PathBuf {
    std::env::var_os("DWSU_REPO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."))
}

/// This build's adopters, probed once for the test binary.
pub(crate) fn build() -> &'static Adopters {
    Adopters::cache_for_test(|| {
        let dir = Box::leak(Box::new(private_tempdir()));
        let adopters = Adopters::probe(dir.path());
        eprintln!(
            "probe: {:?}\n  {}",
            adopters.p9a_status(),
            adopters.observations().join("\n  ")
        );
        adopters
    })
}

pub(crate) fn private_tempdir() -> tempfile::TempDir {
    let dir = tempfile::Builder::new().prefix("dwsu-").tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    dir
}

/// A data root holding one network directory copied from a fixture.
pub(crate) struct Fixture {
    _tmp: tempfile::TempDir,
    pub root: PathBuf,
    pub net: PathBuf,
}

impl Fixture {
    pub fn wallet(&self) -> PathBuf {
        self.net.join("wallet.sqlite")
    }
}

fn copy_tree(src: &Path, dest: &Path) {
    crate::fsx::create_private_dir(dest).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dest.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), &to).unwrap();
        }
    }
}

pub(crate) fn fixture(version: &str) -> Fixture {
    let tmp = private_tempdir();
    let root = tmp.path().join("data");
    crate::fsx::create_private_dir(&root).unwrap();
    let net = root.join("regtest");
    copy_tree(
        &repo_root()
            .join("testdata/upgrade")
            .join(version)
            .join("data/regtest"),
        &net,
    );
    Fixture {
        _tmp: tmp,
        root,
        net,
    }
}

/// Every file and directory under `dir`: relative path → sha256 (or "dir").
pub(crate) fn tree(dir: &Path) -> BTreeMap<String, String> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let rel = entry
                .path()
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if entry.file_type().unwrap().is_dir() {
                out.insert(rel, "dir".into());
                walk(base, &entry.path(), out);
            } else {
                out.insert(rel, crate::sql::sha256_file(&entry.path()).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// The paths whose hash differs between two [`tree`]s, or that only one has.
pub(crate) fn tree_diff(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) -> Vec<String> {
    a.keys()
        .chain(b.keys())
        .filter(|k| a.get(*k) != b.get(*k))
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_795_567_800)
}

pub(crate) fn later(secs: u64) -> SystemTime {
    now() + Duration::from_secs(secs)
}

pub(crate) fn config(net: &Path) -> SqlitePersisterConfig {
    SqlitePersisterConfig::new(net.join("wallet.sqlite"))
        .with_load_policy(LoadPolicy::Strict)
        .with_auto_backup_dir(Some(net.join("backups").join("auto")))
}

/// Open the upstream persister Strict and load every wallet.
pub(crate) fn open_and_load(net: &Path) -> (std::sync::Arc<SqlitePersister>, PersisterOpened) {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let (p, token) = PersisterOpened::open(config(net)).expect("upstream open");
    p.load().expect("upstream load");
    (p, token)
}

/// Table-by-table checksums of a store, minus refinery's `applied_on`.
pub(crate) fn tables(path: &Path) -> BTreeMap<String, String> {
    let conn = Connection::open(path).unwrap();
    crate::sql::table_digests(
        &conn,
        &[
            ("refinery_schema_history", "applied_on"),
            (crate::fork::FORK_HISTORY_RETIRED, "applied_on"),
        ],
    )
    .unwrap()
}

/// `PRAGMA table_info` of `table` as (name, type, notnull, default, pk).
pub(crate) fn columns(
    conn: &Connection,
    table: &str,
) -> Vec<(String, String, i64, Option<String>, i64)> {
    conn.prepare(
        "SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info(?1) ORDER BY cid",
    )
    .unwrap()
    .query_map([table], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

/// A fresh upstream store's path, for shape comparisons.
pub(crate) fn fresh_store() -> (tempfile::TempDir, PathBuf) {
    let dir = private_tempdir();
    let path = dir.path().join("fresh.sqlite");
    drop(
        SqlitePersister::open(
            SqlitePersisterConfig::new(&path).with_auto_backup_dir(Some(dir.path().join("auto"))),
        )
        .unwrap(),
    );
    (dir, path)
}

pub(crate) fn exec(path: &Path, sql: &str) {
    Connection::open(path).unwrap().execute_batch(sql).unwrap();
}

/// A retired contact-pool row, as a fork build rotating a contact writes it.
pub(crate) fn add_retired_row(path: &Path) {
    exec(
        path,
        "INSERT INTO core_address_pool_retired (wallet_id, account_type, account_index, account_xpub, \
           pool_type, address_index, script, used) \
         SELECT wallet_id, 'dashpay_external', 0, x'0488b21e', 2, 0, x'76a914' || randomblob(20) || x'88ac', 1 \
         FROM wallets LIMIT 1",
    );
}
