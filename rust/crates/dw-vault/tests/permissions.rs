//! Modes of the vault directory and file through the public API (review
//! D1-r2): missing parents are created owner-only whatever the umask, and
//! neither a symlink at the vault directory nor one at the temp file's name
//! changes anything outside it.
#![cfg(unix)]

mod common;

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use common::*;
use dw_vault::Vault;
use key_wallet::Network;

// tempfile's directories take the umask's mode; the vault only restricts an
// existing directory below one that others cannot write.
use dw_testutil::private_tempdir;

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777
}

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn create_at(fx: &Fixture, dir: &Path) {
    let v = Vault::open(dir.to_path_buf(), Network::Regtest, "regtest", fx.config()).unwrap();
    v.create(None).unwrap();
}

#[test]
fn missing_parents_and_the_vault_directory_are_owner_only() {
    let fx = Fixture::new();
    let tmp = private_tempdir();
    let dir = tmp.path().join("a/b/vault");
    create_at(&fx, &dir);
    for d in ["a", "a/b", "a/b/vault"] {
        assert_eq!(mode(&tmp.path().join(d)), 0o700, "{d}");
    }
    assert_eq!(mode(&dir.join("vault.dwv")), 0o600);

    // An existing vault directory an older build left open is restricted.
    let older = tmp.path().join("older");
    std::fs::create_dir(&older).unwrap();
    chmod(&older, 0o775);
    create_at(&fx, &older);
    assert_eq!(mode(&older), 0o700);
}

#[test]
fn a_symlinked_vault_directory_keeps_its_targets_mode() {
    let fx = Fixture::new();
    let tmp = private_tempdir();
    let target = tmp.path().join("elsewhere");
    std::fs::create_dir(&target).unwrap();
    chmod(&target, 0o755);
    symlink(&target, tmp.path().join("vault")).unwrap();
    create_at(&fx, &tmp.path().join("vault"));
    assert_eq!(mode(&target), 0o755);
    assert_eq!(mode(&target.join("vault.dwv")), 0o600);
}

#[test]
fn a_symlink_at_the_temp_file_is_replaced_not_followed() {
    let fx = Fixture::new();
    let tmp = private_tempdir();
    let dir = tmp.path().join("vault");
    std::fs::create_dir(&dir).unwrap();
    chmod(&dir, 0o700);
    let outside = tmp.path().join("outside");
    std::fs::write(&outside, b"not the vault's").unwrap();
    chmod(&outside, 0o644);
    symlink(&outside, dir.join("vault.dwv.tmp")).unwrap();

    create_at(&fx, &dir);
    assert_eq!(std::fs::read(&outside).unwrap(), b"not the vault's");
    assert_eq!(mode(&outside), 0o644);
    assert_eq!(mode(&dir.join("vault.dwv")), 0o600);
    assert!(
        std::fs::symlink_metadata(dir.join("vault.dwv"))
            .unwrap()
            .is_file()
    );
}
