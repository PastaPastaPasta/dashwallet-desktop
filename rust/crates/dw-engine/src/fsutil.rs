//! File-system helpers: the owner-only directories and files of the data root
//! ([`dw_fs`], which never changes a mode through a swapped-in symlink).

use std::io;
use std::path::Path;

pub(crate) use dw_fs::create_private_dir;

/// Creates `rel` below the network directory `data_dir` (`<root>/<network>`),
/// both the engine's: each directory from the network directory down is
/// created 0700, or loses its group and other permissions when an older
/// build or a permissive umask left them open. The data root above is the
/// user's and keeps its mode (QT-004).
pub(crate) fn create_owned_dir(data_dir: &Path, rel: &Path) -> io::Result<()> {
    let (root, network) = split(data_dir)?;
    dw_fs::create_owned_dir(root, &Path::new(network).join(rel))
}

/// [`create_owned_dir`] for the file `rel` below the network directory: the
/// engine's files that would otherwise get the umask's mode (`app.sqlite`,
/// whose `-wal` and `-shm` files SQLite gives the database file's mode, and
/// the open-session marker) are created 0600 or restricted to it.
pub(crate) fn create_owned_file(data_dir: &Path, rel: &Path) -> io::Result<()> {
    let (root, network) = split(data_dir)?;
    dw_fs::create_owned_file(root, &Path::new(network).join(rel))
}

fn split(data_dir: &Path) -> io::Result<(&Path, &std::ffi::OsStr)> {
    match (data_dir.parent(), data_dir.file_name()) {
        (Some(root), Some(network)) => Ok((root, network)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a network directory", data_dir.display()),
        )),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn owns_the_network_directory_but_not_the_root() {
        let tmp = dw_testutil::private_tempdir();
        let root = tmp.path().join("root");
        create_private_dir(&root).unwrap();
        chmod(&root, 0o755);
        let net = root.join("regtest");
        std::fs::create_dir(&net).unwrap();
        chmod(&net, 0o775);

        create_owned_dir(&net, Path::new("backups/auto")).unwrap();
        create_owned_file(&net, Path::new("app.sqlite")).unwrap();
        assert_eq!(mode(&root), 0o755);
        assert_eq!(mode(&net), 0o700);
        assert_eq!(mode(&net.join("backups")), 0o700);
        assert_eq!(mode(&net.join("backups/auto")), 0o700);
        assert_eq!(mode(&net.join("app.sqlite")), 0o600);
        assert!(create_owned_dir(Path::new("/"), Path::new("x")).is_err());
    }
}
