//! File-system helpers.

use std::io;
use std::path::{Path, PathBuf};

/// Creates `dir` and any missing parents. Every directory this call creates
/// is restricted to the current user (0700 on Unix) whatever the umask,
/// because SqlitePersister refuses databases under group- or world-writable
/// directories (0775 is the default under the umask 002 of Ubuntu and Fedora
/// desktops).
///
/// Directories that already exist keep their permissions: when the user
/// chose the data root (dash-qt's `--datadir` or first-run chooser, QT-004),
/// the engine must not change the mode of a directory it does not own. The
/// storage check then names the directory that is too open.
pub(crate) fn create_private_dir(dir: &Path) -> io::Result<()> {
    // Collect the missing components, deepest first.
    let mut missing: Vec<PathBuf> = Vec::new();
    let mut cursor = Some(dir);
    while let Some(path) = cursor {
        if path.as_os_str().is_empty() || path.exists() {
            break;
        }
        missing.push(path.to_path_buf());
        cursor = path.parent();
    }
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        // 0700 from the start; the chmod below covers a umask that masks
        // owner bits.
        builder.mode(0o700);
    }
    for path in missing.iter().rev() {
        match builder.create(path) {
            Ok(()) => restrict_to_user(path)?,
            // Another thread or process created it first: it is not ours.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => {}
            Err(e) => return Err(e),
        }
    }
    if !dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("{} is not a directory", dir.display()),
        ));
    }
    Ok(())
}

/// [`create_private_dir`] for a directory the engine owns: a network
/// directory below the data root and the directories in it. One that already
/// exists with group or other permissions (created by an older build, or
/// under a permissive umask) loses them, and the change is logged.
pub(crate) fn create_owned_dir(dir: &Path) -> io::Result<()> {
    create_private_dir(dir)?;
    tighten_owned(dir)
}

/// Creates the file `path` owner-only (0600 on Unix) if it is missing, or
/// takes group and other permissions off an existing one and logs it. For
/// the engine's own files that would otherwise get the umask's mode
/// (`app.sqlite`, whose `-wal` and `-shm` files SQLite gives the database
/// file's mode, and the open-session marker).
pub(crate) fn create_owned_file(path: &Path) -> io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    drop(opts.open(path)?);
    tighten_owned(path)
}

/// Takes the group and other permissions off the existing file or directory
/// `path`, and logs the change. A symlink is left alone: it was put there on
/// purpose, and the storage check follows it to its target anyway.
#[cfg(unix)]
fn tighten_owned(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(path)?;
    let old = meta.permissions().mode() & 0o7777;
    if meta.file_type().is_symlink() || old & 0o077 == 0 {
        return Ok(());
    }
    let mode = old & !0o077;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    tracing::warn!(
        path = %path.display(),
        "restricted to the current user: mode was {old:04o}, now {mode:04o}"
    );
    Ok(())
}

#[cfg(not(unix))]
fn tighten_owned(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn restrict_to_user(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict_to_user(_path: &Path) -> io::Result<()> {
    // Windows: a directory under the user profile inherits a per-user ACL.
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn restricts_only_the_directories_it_creates() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("chosen-root");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();

        let net = root.join("regtest").join("spv");
        create_private_dir(&net).unwrap();

        assert_eq!(
            mode(&root),
            0o755,
            "an existing user-chosen root keeps its mode"
        );
        assert_eq!(mode(&root.join("regtest")), 0o700);
        assert_eq!(mode(&net), 0o700);
    }

    #[test]
    fn leaves_an_existing_directory_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("existing");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o750)).unwrap();
        create_private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o750);
    }

    #[test]
    fn restricts_an_existing_owned_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o775)).unwrap();
        let net = root.join("regtest");
        std::fs::create_dir(&net).unwrap();
        std::fs::set_permissions(&net, std::fs::Permissions::from_mode(0o775)).unwrap();

        create_owned_dir(&net).unwrap();
        assert_eq!(mode(&net), 0o700);
        assert_eq!(mode(&root), 0o775, "the parent is not the engine's");

        // Only group and other bits go: 0555 becomes 0500, not 0700.
        std::fs::set_permissions(&net, std::fs::Permissions::from_mode(0o555)).unwrap();
        create_owned_dir(&net).unwrap();
        assert_eq!(mode(&net), 0o500);
        std::fs::set_permissions(&net, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn leaves_a_symlinked_owned_directory_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = tmp.path().join("regtest");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        create_owned_dir(&link).unwrap();
        assert_eq!(mode(&target), 0o755);
    }

    #[test]
    fn creates_and_restricts_an_owned_file() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("app.sqlite");
        create_owned_file(&db).unwrap();
        assert_eq!(mode(&db), 0o600);

        std::fs::write(&db, b"kept").unwrap();
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o664)).unwrap();
        create_owned_file(&db).unwrap();
        assert_eq!(mode(&db), 0o600);
        assert_eq!(
            std::fs::read(&db).unwrap(),
            b"kept",
            "contents are left alone"
        );
    }

    #[test]
    fn rejects_a_file_in_the_way() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("file");
        std::fs::write(&file, b"x").unwrap();
        assert!(create_private_dir(&file).is_err());
        assert!(create_owned_dir(&file).is_err());
    }
}
