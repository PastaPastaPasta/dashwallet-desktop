//! File-system helpers.

use std::io;
use std::path::{Path, PathBuf};

/// Creates `dir` and any missing parents. Every directory this call creates
/// is restricted to the current user (0700 on Unix), because SqlitePersister
/// refuses databases under group- or world-writable directories.
///
/// Directories that already exist keep their permissions: when the user
/// chose the data root (dash-qt's `--datadir` or first-run chooser, QT-004),
/// the engine must not change the mode of a directory it does not own.
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
    for path in missing.iter().rev() {
        match std::fs::create_dir(path) {
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
    fn rejects_a_file_in_the_way() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("file");
        std::fs::write(&file, b"x").unwrap();
        assert!(create_private_dir(&file).is_err());
    }
}
