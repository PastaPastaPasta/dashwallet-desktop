//! Which folder under the data root holds each network's data. By default a
//! network lives in `<data root>/<dir name>` ([`DashNetwork::dir_name`]).
//! After the way out of a refused open (Q107 C1), the network starts over in
//! a new folder beside it, `<dir name>-recovered-<n>`, and
//! `<data root>/network-folders.json` (`{ "<dir name>": "<folder>" }`) names
//! it. The refused folder is left as it is.
//!
//! A missing record means every network uses its default folder. A record
//! that can't be read, doesn't parse, names anything but a
//! `<dir name>-recovered-<n>` folder, or names one that isn't there fails
//! closed: the network's folder is an error, never the default one. So does
//! a network whose default folder is another network's recorded one (a
//! devnet named like a recovery folder). A write replaces the record
//! atomically and durably, after the new folder is durable.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::{DashNetwork, EngineError};
use dw_store_upgrade::RESTORED_MARK;

/// The record's name in the data root.
pub(crate) const NETWORK_FOLDERS_FILE: &str = "network-folders.json";

const RECOVERED: &str = "-recovered-";

/// The highest `n` a recovery folder may take.
const MAX_RECOVERED: u32 = 10_000;

type Folders = BTreeMap<String, String>;

fn record_path(root: &Path) -> PathBuf {
    root.join(NETWORK_FOLDERS_FILE)
}

fn damaged(detail: impl std::fmt::Display) -> EngineError {
    EngineError::Storage(format!("{NETWORK_FOLDERS_FILE} can't be used: {detail}"))
}

/// Whether `folder` is `<dir_name>-recovered-<n>`, `n` ≥ 1 written without
/// leading zeros.
fn is_recovery_folder(dir_name: &str, folder: &str) -> bool {
    folder
        .strip_prefix(dir_name)
        .and_then(|rest| rest.strip_prefix(RECOVERED))
        .and_then(|n| n.parse::<u32>().ok().filter(|v| v.to_string() == n))
        .is_some_and(|n| (1..=MAX_RECOVERED).contains(&n))
}

/// The record in `root`: empty when there is none.
fn read(root: &Path) -> Result<Folders, EngineError> {
    let bytes = match std::fs::read(record_path(root)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Folders::new()),
        Err(e) => return Err(damaged(e)),
    };
    let folders: Folders = serde_json::from_slice(&bytes).map_err(damaged)?;
    let plain = |name: &str| {
        !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', '\0'])
    };
    for (network, folder) in &folders {
        if !plain(network) || !is_recovery_folder(network, folder) {
            return Err(damaged(format!("{network:?} names {folder:?}")));
        }
    }
    Ok(folders)
}

/// The folder of `network` under `root`.
pub(crate) fn resolve(root: &Path, network: &DashNetwork) -> Result<PathBuf, EngineError> {
    let name = network.dir_name();
    let folders = read(root)?;
    let Some(folder) = folders.get(&name) else {
        if folders.values().any(|folder| *folder == name) {
            return Err(damaged(format!("{name:?} is another network's folder")));
        }
        return Ok(root.join(name));
    };
    let path = root.join(folder);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_dir() => Ok(path),
        Ok(_) => Err(damaged(format!("{folder:?} isn't a folder"))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            Err(damaged(format!("{folder:?} is missing")))
        }
        Err(e) => Err(damaged(e)),
    }
}

/// Every redirected network's dir name with its folder's name.
pub(crate) fn redirects(root: &Path) -> Result<Folders, EngineError> {
    read(root)
}

/// Creates `<dir name>-recovered-<n>` beside the network's folders, the
/// lowest `n` free, owner-only, with the restore's RESTORED mark in it, and
/// then records it as the network's folder. Writes only the new folder and
/// the record: nothing in the network's current folder.
pub(crate) fn recover(root: &Path, network: &DashNetwork) -> Result<PathBuf, EngineError> {
    let name = network.dir_name();
    let mut folders = read(root)?;
    let left = folders.get(&name).unwrap_or(&name).clone();
    let mut created = None;
    for n in 1..=MAX_RECOVERED {
        let folder = format!("{name}{RECOVERED}{n}");
        let path = root.join(&folder);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => continue,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        match create_owner_only_dir(&path) {
            Ok(()) => {
                created = Some((folder, path));
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let Some((folder, path)) = created else {
        return Err(EngineError::Storage(format!(
            "no free {name}{RECOVERED}<n> folder up to {MAX_RECOVERED}"
        )));
    };
    folders.insert(name, folder);
    let written = write_restored_mark(&path, &left)
        .and_then(|()| sync_dir(root))
        .and_then(|()| write(root, &folders));
    if let Err(e) = written {
        let backups = path.join(dw_store_upgrade::UPGRADE_BACKUPS_DIR);
        let _ = std::fs::remove_file(backups.join(RESTORED_MARK));
        let _ = std::fs::remove_dir(&backups);
        let _ = std::fs::remove_dir(&path);
        return Err(e);
    }
    let sync_redirect = || {
        #[cfg(test)]
        if FAIL_FINAL_SYNC.get() {
            return Err(io::Error::other("injected at the final sync").into());
        }
        sync_dir(root)
    };
    sync_redirect().map_err(|e| {
        EngineError::RetryableIo(format!(
            "{NETWORK_FOLDERS_FILE} renamed but its folder not synced: {e}"
        ))
    })?;
    Ok(path)
}

fn sync_dir(dir: &Path) -> Result<(), EngineError> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Sets dw-store-upgrade's money-gate mark ([`RESTORED_MARK`]) in the new
/// folder `dir`, in the crate's format (it has no public writer), with no
/// pending payment: the folder holds no store whose payments could be held.
/// `left` names the folder the network left.
fn write_restored_mark(dir: &Path, left: &str) -> Result<(), EngineError> {
    use std::io::Write;
    let backups = dir.join(dw_store_upgrade::UPGRADE_BACKUPS_DIR);
    create_owner_only_dir(&backups)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let stamp = crate::csv_export::iso_local(now, 0).replace(['-', ':'], "");
    let text = format!("dw-store-upgrade restored\nformat 1\nrestored {stamp}Z from {left}\n");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(backups.join(RESTORED_MARK))?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        std::fs::File::open(&backups)?.sync_all()?;
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

fn create_owner_only_dir(path: &Path) -> io::Result<()> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

#[cfg(test)]
thread_local! {
    static FAIL_BEFORE_RENAME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAIL_FINAL_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Replaces the record: a temporary file of this write's own in `root`,
/// synced and renamed over the record; the caller syncs `root`. A failed
/// write leaves the record as it was and no temporary file.
fn write(root: &Path, folders: &Folders) -> Result<(), EngineError> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static WRITES: AtomicU64 = AtomicU64::new(0);

    let json =
        serde_json::to_vec_pretty(folders).map_err(|e| EngineError::Internal(e.to_string()))?;
    let tmp = root.join(format!(
        "{NETWORK_FOLDERS_FILE}.{}.{}.tmp",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(&json)?;
        file.sync_all()?;
        #[cfg(test)]
        if FAIL_BEFORE_RENAME.get() {
            return Err(io::Error::other("injected before the rename"));
        }
        std::fs::rename(&tmp, record_path(root))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn no_record_is_the_default_folder() {
        let root = dw_testutil::private_tempdir();
        for network in [
            DashNetwork::Mainnet,
            DashNetwork::Testnet,
            DashNetwork::Regtest,
            DashNetwork::Devnet { name: "p".into() },
        ] {
            assert_eq!(
                resolve(root.path(), &network).unwrap(),
                root.path().join(network.dir_name())
            );
        }
    }

    #[test]
    fn a_recovery_folder_is_the_networks_and_no_others() {
        let root = dw_testutil::private_tempdir();
        let dir = recover(root.path(), &DashNetwork::Testnet).unwrap();
        assert_eq!(dir, root.path().join("testnet-recovered-1"));
        assert_eq!(resolve(root.path(), &DashNetwork::Testnet).unwrap(), dir);
        assert_eq!(
            resolve(root.path(), &DashNetwork::Mainnet).unwrap(),
            root.path().join("mainnet")
        );
        assert_eq!(
            std::fs::read_to_string(record_path(root.path())).unwrap(),
            "{\n  \"testnet\": \"testnet-recovered-1\"\n}"
        );
        assert_eq!(names(&dir), [dw_store_upgrade::UPGRADE_BACKUPS_DIR]);
        let backups = dir.join(dw_store_upgrade::UPGRADE_BACKUPS_DIR);
        assert_eq!(names(&backups), [RESTORED_MARK]);
        let mark = std::fs::read_to_string(backups.join(RESTORED_MARK)).unwrap();
        let lines: Vec<&str> = mark.lines().collect();
        assert_eq!(lines[..2], ["dw-store-upgrade restored", "format 1"]);
        assert!(lines[2].starts_with("restored 20"), "{mark}");
        assert!(lines[2].ends_with("Z from testnet"), "{mark}");
        assert_eq!(lines.len(), 3, "no pending payment: {mark}");
    }

    #[cfg(unix)]
    #[test]
    fn the_recovery_folder_is_owner_only_whatever_the_umask() {
        use std::os::unix::fs::PermissionsExt;
        let root = dw_testutil::private_tempdir();
        let dir = recover(root.path(), &DashNetwork::Regtest).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn n_skips_a_name_already_taken() {
        let root = dw_testutil::private_tempdir();
        std::fs::create_dir(root.path().join("mainnet-recovered-1")).unwrap();
        std::fs::write(root.path().join("mainnet-recovered-2"), b"a file").unwrap();
        let dir = recover(root.path(), &DashNetwork::Mainnet).unwrap();
        assert_eq!(dir, root.path().join("mainnet-recovered-3"));
        assert_eq!(resolve(root.path(), &DashNetwork::Mainnet).unwrap(), dir);
        assert_eq!(
            std::fs::read(root.path().join("mainnet-recovered-2")).unwrap(),
            b"a file"
        );
        let again = recover(root.path(), &DashNetwork::Mainnet).unwrap();
        assert_eq!(again, root.path().join("mainnet-recovered-4"));
    }

    #[cfg(unix)]
    #[test]
    fn n_skips_a_dangling_symlink() {
        let root = dw_testutil::private_tempdir();
        std::os::unix::fs::symlink(
            root.path().join("nowhere"),
            root.path().join("regtest-recovered-1"),
        )
        .unwrap();
        let dir = recover(root.path(), &DashNetwork::Regtest).unwrap();
        assert_eq!(dir, root.path().join("regtest-recovered-2"));
        assert!(!root.path().join("nowhere").exists());
    }

    #[test]
    fn a_damaged_record_fails_closed() {
        let root = dw_testutil::private_tempdir();
        let cases: [&[u8]; 9] = [
            b"",
            b"{\"mainnet\": ",
            b"[]",
            b"{\"mainnet\": 1}",
            b"{\"mainnet\": \"../elsewhere\"}",
            b"{\"mainnet\": \"testnet-recovered-1\"}",
            b"{\"mainnet\": \"mainnet-recovered-0\"}",
            b"{\"mainnet\": \"mainnet-recovered-01\"}",
            b"{\"mainnet\": \"mainnet\"}",
        ];
        for bytes in cases {
            std::fs::write(record_path(root.path()), bytes).unwrap();
            let text = String::from_utf8_lossy(bytes);
            for network in [DashNetwork::Mainnet, DashNetwork::Testnet] {
                let e = resolve(root.path(), &network).expect_err(&text);
                assert!(matches!(e, EngineError::Storage(_)), "{text}: {e:?}");
            }
            assert!(
                recover(root.path(), &DashNetwork::Mainnet).is_err(),
                "{text}"
            );
        }
        assert_eq!(names(root.path()), [NETWORK_FOLDERS_FILE]);
    }

    #[test]
    fn a_recorded_folder_that_is_gone_fails_closed() {
        let root = dw_testutil::private_tempdir();
        let dir = recover(root.path(), &DashNetwork::Testnet).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let e = resolve(root.path(), &DashNetwork::Testnet).unwrap_err();
        assert!(matches!(e, EngineError::Storage(_)), "{e:?}");
        std::fs::write(&dir, b"not a folder").unwrap();
        let e = resolve(root.path(), &DashNetwork::Testnet).unwrap_err();
        assert!(matches!(e, EngineError::Storage(_)), "{e:?}");
        assert_eq!(
            resolve(root.path(), &DashNetwork::Mainnet).unwrap(),
            root.path().join("mainnet")
        );
    }

    #[test]
    fn a_devnet_named_like_a_recovery_folder_fails_closed() {
        let root = dw_testutil::private_tempdir();
        let a = DashNetwork::Devnet { name: "a".into() };
        let dir = recover(root.path(), &a).unwrap();
        assert_eq!(dir, root.path().join("devnet-a-recovered-1"));
        let twin = DashNetwork::Devnet {
            name: "a-recovered-1".into(),
        };
        assert_eq!(twin.dir_name(), "devnet-a-recovered-1");
        let e = resolve(root.path(), &twin).unwrap_err();
        assert!(matches!(e, EngineError::Storage(_)), "{e:?}");
        assert_eq!(resolve(root.path(), &a).unwrap(), dir);
    }

    #[test]
    fn an_unreadable_record_fails_closed() {
        let root = dw_testutil::private_tempdir();
        std::fs::create_dir(record_path(root.path())).unwrap();
        let e = resolve(root.path(), &DashNetwork::Mainnet).unwrap_err();
        assert!(matches!(e, EngineError::Storage(_)), "{e:?}");
        assert!(recover(root.path(), &DashNetwork::Mainnet).is_err());
    }

    #[test]
    fn a_failed_write_leaves_the_record_and_no_temporary_file() {
        let root = dw_testutil::private_tempdir();
        recover(root.path(), &DashNetwork::Mainnet).unwrap();
        let before = std::fs::read(record_path(root.path())).unwrap();
        assert_eq!(
            names(root.path()),
            ["mainnet-recovered-1", NETWORK_FOLDERS_FILE]
        );

        FAIL_BEFORE_RENAME.set(true);
        let failed = recover(root.path(), &DashNetwork::Testnet);
        FAIL_BEFORE_RENAME.set(false);
        assert!(failed.is_err());
        assert_eq!(std::fs::read(record_path(root.path())).unwrap(), before);
        assert_eq!(
            names(root.path()),
            ["mainnet-recovered-1", NETWORK_FOLDERS_FILE],
            "the record is whole, and no temporary file or unrecorded folder is left"
        );
        assert_eq!(
            resolve(root.path(), &DashNetwork::Testnet).unwrap(),
            root.path().join("testnet")
        );

        let dir = recover(root.path(), &DashNetwork::Testnet).unwrap();
        assert_eq!(dir, root.path().join("testnet-recovered-1"));
        assert_eq!(
            resolve(root.path(), &DashNetwork::Mainnet).unwrap(),
            root.path().join("mainnet-recovered-1")
        );
    }

    #[test]
    fn a_failed_final_sync_is_a_retryable_error() {
        let root = dw_testutil::private_tempdir();
        FAIL_FINAL_SYNC.set(true);
        let failed = recover(root.path(), &DashNetwork::Testnet);
        FAIL_FINAL_SYNC.set(false);
        let e = failed.unwrap_err();
        assert!(matches!(e, EngineError::RetryableIo(_)), "{e:?}");
        assert_eq!(
            resolve(root.path(), &DashNetwork::Testnet).unwrap(),
            root.path().join("testnet-recovered-1"),
            "the record already names the folder: it stays"
        );
        let dir = recover(root.path(), &DashNetwork::Testnet).unwrap();
        assert_eq!(dir, root.path().join("testnet-recovered-2"));
    }

    #[test]
    fn a_devnet_folder_names_its_devnet() {
        let root = dw_testutil::private_tempdir();
        let net = DashNetwork::Devnet {
            name: "paloma".into(),
        };
        let dir = recover(root.path(), &net).unwrap();
        assert_eq!(dir, root.path().join("devnet-paloma-recovered-1"));
        assert_eq!(resolve(root.path(), &net).unwrap(), dir);
        assert_eq!(
            redirects(root.path()).unwrap(),
            Folders::from([("devnet-paloma".into(), "devnet-paloma-recovered-1".into())])
        );
    }
}
