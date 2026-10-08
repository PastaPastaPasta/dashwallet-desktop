//! Test helpers shared by the workspace's test suites. Dev-dependency only.

/// A temporary directory that only the current user can write to, whatever
/// the process umask.
///
/// `SqlitePersister` refuses a database when any directory above it is group-
/// or world-writable without the sticky bit, and `tempfile::tempdir()` creates
/// its directory with the umask's mode: 0775 under Ubuntu's default umask 002,
/// which fails every engine test.
pub fn private_tempdir() -> tempfile::TempDir {
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().expect("temp dir")
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn private_tempdir_is_owner_only_under_a_permissive_umask() {
        // SAFETY: umask only swaps the process file-mode mask and cannot fail;
        // this crate has no other test that creates files meanwhile.
        let old = unsafe { libc::umask(0o002) };
        let dir = super::private_tempdir();
        unsafe { libc::umask(old) };
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
}
