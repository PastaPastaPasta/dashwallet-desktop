//! Owner-only directories and files for the wallet's data.
//!
//! The wallet storage refuses a database below a group- or world-writable
//! directory, and `mkdir` takes the umask's mode: 0775 under the umask 002 of
//! Ubuntu and Fedora desktops (user-private groups). The engine and the vault
//! therefore create their directories 0700 and their files 0600, and take
//! group and other access off the ones they own when an older build or a
//! permissive umask left them open.
//!
//! A mode is never changed by path: a `chmod` changes whatever is at the path
//! when it runs, and someone who can write a directory on the way can swap the
//! checked entry for a symlink in between (review D1-r2). Every path is walked
//! one component at a time from `/` instead, each directory opened with
//! `openat(O_DIRECTORY | O_NOFOLLOW)` relative to its parent's descriptor, and
//! a mode is read with `fstat` and changed with `fchmod` on that descriptor.
//! On Linux the walk opens directories with `O_PATH`, so a parent the user
//! may only search (`/home` at 0711) is no obstacle; a directory is reopened
//! through its own descriptor (`openat(fd, ".")`) only to change its mode.
//! Symlinks on the way are resolved here, by the same rules.
//!
//! Trusted owners are the current user, root, and the owner of `/` (root seen
//! through a user namespace, as in Flatpak), as in the storage's own ancestor
//! check. A mode changes only when
//! - no directory on the way lets someone untrusted replace its entries: one
//!   an untrusted user owns, or that is group- or other-writable without the
//!   sticky bit (with it, when the entry's owner is untrusted),
//! - no symlink was followed in the part of the path the app owns (its target
//!   is the user's choice, and is left as it is), and a file has no other
//!   hard link, and
//! - the item belongs to the current user. One of the app's that belongs to
//!   another user is an error naming it, and so is a directory or symlink in
//!   the app's part that an untrusted user owns.
//!
//! Otherwise the item is left as it is and a warning logged; the storage check
//! then names the directory that is too open.
//!
//! Windows keeps its per-user ACLs under the profile: there these functions
//! only create what is missing.

use std::io;
use std::path::Path;

/// Creates `dir` and any missing parents, each 0700 whatever the umask.
///
/// Directories that already exist keep their mode: when the user chose the
/// data root (dash-qt's `--datadir` or the first-run chooser, QT-004), it and
/// its parents are theirs, and the storage check names one that is too open.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    create_owned_dir(dir, Path::new(""))
}

/// Creates `base` as [`create_private_dir`] does, then the directories of the
/// relative path `owned` below it, which are the app's: a missing one is
/// created 0700, an existing one loses its group and other permissions (and
/// the change is logged). A symlink among them is followed, but nothing at or
/// below it changes.
pub fn create_owned_dir(base: &Path, owned: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        unix::Walk::new()?.run(base, owned)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(base.join(owned))
    }
}

/// [`create_owned_dir`] for the parent of `owned`, then the file itself:
/// created empty and 0600 when missing, otherwise its group and other
/// permissions go and its contents stay. A symlink there is left alone.
pub fn create_owned_file(base: &Path, owned: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        unix::owned_file(base, owned)
    }
    #[cfg(not(unix))]
    {
        let path = base.join(owned);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .map(drop)
    }
}

#[cfg(unix)]
mod unix {
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::os::fd::OwnedFd;
    use std::os::unix::ffi::OsStringExt;
    use std::path::{Component, Path, PathBuf};

    use rustix::fs::{AtFlags, FileType, Mode, OFlags, RawMode, Stat};
    use rustix::io::Errno;

    /// Symlinks one walk follows before giving up (the kernel's limit).
    const MAX_SYMLINKS: usize = 40;
    const STICKY: u32 = 0o1000;
    /// `O_PATH` needs only search permission on the directory; elsewhere a
    /// directory the user may not read cannot be walked.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const DIR_FLAGS: OFlags = OFlags::PATH
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const DIR_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Part {
        /// The directory the caller names and its parents: created when
        /// missing, otherwise never changed.
        Chosen,
        /// The app's own directories below it.
        Owned,
    }

    /// One component still to walk.
    enum Step {
        /// Back to `/`: an absolute symlink target.
        Root,
        /// `..`: back to the directory the current one was opened from.
        Up,
        Name(OsString, Part),
    }

    /// An open directory on the walked path, or the file at its end.
    pub(super) struct Item {
        fd: OwnedFd,
        stat: Stat,
        path: PathBuf,
        /// No other user could have replaced an entry from `/` to here.
        safe: bool,
    }

    /// The open directories from `/` to the current one. Each is the actual
    /// child of the one before it, so `..` pops one, as in the kernel.
    pub(super) struct Walk {
        dirs: Vec<Item>,
        euid: u32,
        /// The owner of `/`: root, or the uid root maps to in a user namespace.
        root_uid: u32,
        links: usize,
        /// A symlink was followed that another user could have put there.
        unsafe_link: bool,
        /// A symlink was followed in the owned part.
        owned_link: bool,
    }

    impl Walk {
        pub(super) fn new() -> io::Result<Self> {
            let fd = rustix::fs::open("/", DIR_FLAGS, Mode::empty())?;
            let stat = rustix::fs::fstat(&fd)?;
            Ok(Self {
                root_uid: uid(&stat),
                dirs: vec![Item {
                    fd,
                    stat,
                    path: PathBuf::from("/"),
                    safe: true,
                }],
                euid: rustix::process::geteuid().as_raw(),
                links: 0,
                unsafe_link: false,
                owned_link: false,
            })
        }

        fn current(&self) -> &Item {
            self.dirs.last().expect("/ stays open")
        }

        /// Walks to `base`, then to `owned` below it.
        pub(super) fn run(&mut self, base: &Path, owned: &Path) -> io::Result<()> {
            let base = if base.as_os_str().is_empty() {
                std::env::current_dir()?
            } else {
                std::path::absolute(base)?
            };
            if !owned
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is not a plain relative path", owned.display()),
                ));
            }
            // A stack: the next step is last.
            let mut steps = Vec::new();
            push_steps(&mut steps, owned, Part::Owned);
            push_steps(&mut steps, &base, Part::Chosen);
            while let Some(step) = steps.pop() {
                match step {
                    Step::Root => self.dirs.truncate(1),
                    Step::Up => {
                        if self.dirs.len() > 1 {
                            self.dirs.pop();
                        }
                    }
                    Step::Name(name, part) => {
                        // A symlink's target is the user's choice, wherever it is.
                        if let Some(target) = self.enter(&name, part)? {
                            push_steps(&mut steps, &target, Part::Chosen);
                        }
                    }
                }
            }
            Ok(())
        }

        /// Opens the directory `name` in the current one, creating it 0700
        /// when missing, and makes it current. For a symlink, returns its
        /// target instead.
        fn enter(&mut self, name: &OsStr, part: Part) -> io::Result<Option<PathBuf>> {
            let parent = self.current();
            let path = parent.path.join(name);
            let mut created = false;
            // The app's own entry, not one behind an owned symlink.
            let owned = part == Part::Owned && !self.owned_link;
            // Another process creating or removing it meanwhile gets a retry.
            for _ in 0..3 {
                match rustix::fs::openat(&parent.fd, name, DIR_FLAGS, Mode::empty()) {
                    Ok(fd) => {
                        let stat = rustix::fs::fstat(&fd)?;
                        if owned {
                            self.check_owner(&path, uid(&stat))?;
                        }
                        let mut dir = Item {
                            safe: parent.safe && !self.replaceable(&parent.stat, uid(&stat)),
                            fd,
                            stat,
                            path,
                        };
                        opened(&dir.path);
                        let mode = mode(&dir.stat);
                        let wanted = match (created, part) {
                            // mkdirat gave 0700 less the umask.
                            (true, _) => 0o700,
                            (false, Part::Owned) => mode & !0o077,
                            (false, Part::Chosen) => mode,
                        };
                        if self.set_mode(&dir, wanted, created)? {
                            // Closed to others now: its entries are stable.
                            dir.stat = rustix::fs::fstat(&dir.fd)?;
                        }
                        self.dirs.push(dir);
                        return Ok(None);
                    }
                    Err(Errno::NOENT) if !created => {
                        match rustix::fs::mkdirat(&parent.fd, name, raw_mode(0o700)) {
                            Ok(()) => created = true,
                            Err(Errno::EXIST) => {}
                            Err(e) => return Err(at(&path, e)),
                        }
                    }
                    // O_NOFOLLOW refuses a symlink with ELOOP (EMLINK on the
                    // BSDs); O_DIRECTORY refuses anything else with ENOTDIR.
                    Err(Errno::LOOP | Errno::MLINK | Errno::NOTDIR) => {
                        let link = rustix::fs::statat(&parent.fd, name, AtFlags::SYMLINK_NOFOLLOW)
                            .map_err(|e| at(&path, e))?;
                        if file_type(&link) != FileType::Symlink {
                            return Err(io::Error::new(
                                io::ErrorKind::NotADirectory,
                                format!("{} is not a directory", path.display()),
                            ));
                        }
                        if owned {
                            self.check_owner(&path, uid(&link))?;
                        }
                        let target = rustix::fs::readlinkat(&parent.fd, name, Vec::new())
                            .map_err(|e| at(&path, e))?;
                        let unsafe_link =
                            !parent.safe || self.replaceable(&parent.stat, uid(&link));
                        self.links += 1;
                        if self.links > MAX_SYMLINKS {
                            return Err(at(&path, Errno::LOOP));
                        }
                        self.unsafe_link |= unsafe_link;
                        if owned {
                            self.owned_link = true;
                            tracing::info!(
                                path = %path.display(),
                                "a symlink: nothing at or below it has its mode changed"
                            );
                        }
                        return Ok(Some(PathBuf::from(OsString::from_vec(target.into_bytes()))));
                    }
                    Err(e) => return Err(at(&path, e)),
                }
            }
            Err(io::Error::other(format!(
                "{} keeps appearing and disappearing",
                path.display()
            )))
        }

        /// Gives `item` the mode `wanted` when it differs and the module rules
        /// allow it; logs what it leaves. Whether it changed it.
        fn set_mode(&self, item: &Item, wanted: u32, created: bool) -> io::Result<bool> {
            let Item {
                fd,
                stat,
                path,
                safe,
            } = item;
            let old = mode(stat);
            // Below an owned symlink: logged once, when it was followed.
            if wanted == old || self.owned_link {
                return Ok(false);
            }
            if !safe || self.unsafe_link {
                tracing::warn!(
                    path = %path.display(),
                    "left at mode {old:04o}: it may not be the app's (another user can replace \
                     an entry on the way, or it has another hard link)"
                );
                return Ok(false);
            }
            if uid(stat) != self.euid {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "{} (mode {old:04o}) belongs to uid {}, not the current user",
                        path.display(),
                        uid(stat)
                    ),
                ));
            }
            // The walk's O_PATH descriptor cannot change a mode: reopen the
            // very same directory through it.
            let reopened;
            let fd = if file_type(stat) == FileType::Directory {
                reopened = rustix::fs::openat(
                    fd,
                    ".",
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|e| at(path, e))?;
                &reopened
            } else {
                fd
            };
            rustix::fs::fchmod(fd, raw_mode(wanted)).map_err(|e| at(path, e))?;
            if !created {
                tracing::warn!(
                    path = %path.display(),
                    "restricted to the current user: mode was {old:04o}, now {wanted:04o}"
                );
            }
            Ok(true)
        }

        fn trusted(&self, uid: u32) -> bool {
            uid == self.euid || uid == 0 || uid == self.root_uid
        }

        /// Whether an untrusted user could replace the entry of `entry_uid`
        /// in the directory `dir`.
        fn replaceable(&self, dir: &Stat, entry_uid: u32) -> bool {
            let mode = mode(dir);
            !self.trusted(uid(dir))
                || (mode & 0o022 != 0 && (mode & STICKY == 0 || !self.trusted(entry_uid)))
        }

        /// The app's own directories and symlinks are never an untrusted
        /// user's: one could only have been planted while a directory above
        /// was open to others.
        fn check_owner(&self, path: &Path, owner: u32) -> io::Result<()> {
            if self.trusted(owner) {
                return Ok(());
            }
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} belongs to uid {owner}, not the current user",
                    path.display()
                ),
            ))
        }
    }

    pub(super) fn owned_file(base: &Path, owned: &Path) -> io::Result<()> {
        let name = owned.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} names no file", owned.display()),
            )
        })?;
        let mut walk = Walk::new()?;
        walk.run(base, owned.parent().unwrap_or(Path::new("")))?;
        let dir = walk.current();
        let path = dir.path.join(name);
        // O_NONBLOCK: a FIFO put there must not hang the open.
        let flags =
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK | OFlags::NOCTTY;
        let create = flags | OFlags::CREATE | OFlags::EXCL;
        let (fd, created) = match rustix::fs::openat(&dir.fd, name, create, raw_mode(0o600)) {
            Ok(fd) => (fd, true),
            Err(Errno::EXIST) => match rustix::fs::openat(&dir.fd, name, flags, Mode::empty()) {
                Ok(fd) => (fd, false),
                // A symlink: whoever put it there chose its target.
                Err(Errno::LOOP | Errno::MLINK) => return Ok(()),
                Err(e) => return Err(at(&path, e)),
            },
            Err(e) => return Err(at(&path, e)),
        };
        let stat = rustix::fs::fstat(&fd)?;
        opened(&path);
        if file_type(&stat) != FileType::RegularFile {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a regular file", path.display()),
            ));
        }
        let wanted = if created { 0o600 } else { mode(&stat) & !0o077 };
        let file = Item {
            safe: dir.safe && !walk.replaceable(&dir.stat, uid(&stat)) && stat.st_nlink == 1,
            fd,
            stat,
            path,
        };
        walk.set_mode(&file, wanted, created).map(drop)
    }

    /// Puts the components of `path` on top of `steps`, first one last.
    fn push_steps(steps: &mut Vec<Step>, path: &Path, part: Part) {
        steps.extend(
            path.components()
                .rev()
                .filter_map(|component| match component {
                    Component::RootDir => Some(Step::Root),
                    Component::ParentDir => Some(Step::Up),
                    Component::Normal(name) => Some(Step::Name(name.to_os_string(), part)),
                    Component::CurDir | Component::Prefix(_) => None,
                }),
        );
    }

    fn at(path: &Path, e: Errno) -> io::Error {
        let e = io::Error::from(e);
        io::Error::new(e.kind(), format!("{}: {e}", path.display()))
    }

    #[allow(clippy::unnecessary_cast)] // u16 on macOS
    fn raw_mode(mode: u32) -> Mode {
        Mode::from_raw_mode(mode as RawMode)
    }

    #[allow(clippy::unnecessary_cast)]
    fn mode(stat: &Stat) -> u32 {
        stat.st_mode as u32 & 0o7777
    }

    #[allow(clippy::unnecessary_cast)]
    fn file_type(stat: &Stat) -> FileType {
        FileType::from_raw_mode(stat.st_mode as RawMode)
    }

    #[allow(clippy::unnecessary_cast)]
    fn uid(stat: &Stat) -> u32 {
        stat.st_uid as u32
    }

    #[cfg(test)]
    pub(crate) type Hook = Box<dyn FnMut(&Path)>;

    #[cfg(test)]
    thread_local! {
        /// Runs after each item is opened, before its mode is looked at: the
        /// tests swap the path for a symlink there.
        pub(crate) static OPENED: std::cell::RefCell<Option<Hook>> =
            const { std::cell::RefCell::new(None) };
    }

    fn opened(_path: &Path) {
        #[cfg(test)]
        OPENED.with(|hook| {
            if let Some(hook) = hook.borrow_mut().as_mut() {
                hook(_path)
            }
        });
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::{Path, PathBuf};

    use super::*;

    // tempfile's directories take the umask's mode, and a group-writable one
    // would make everything below it unsafe to change.
    use dw_testutil::private_tempdir;

    fn mode(path: &Path) -> u32 {
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    }

    fn mkdir(path: &Path, mode: u32) -> PathBuf {
        std::fs::create_dir(path).unwrap();
        chmod(path, mode);
        path.to_path_buf()
    }

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Calls `swap` once, right after `when` is opened.
    fn on_open(when: PathBuf, swap: impl FnOnce() + 'static) {
        let mut swap = Some(swap);
        unix::OPENED.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |path: &Path| {
                if path == when
                    && let Some(swap) = swap.take()
                {
                    swap();
                }
            }));
        });
    }

    #[test]
    fn restricts_only_the_directories_it_creates() {
        let tmp = private_tempdir();
        let root = mkdir(&tmp.path().join("chosen-root"), 0o755);

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
        let tmp = private_tempdir();
        let dir = mkdir(&tmp.path().join("existing"), 0o750);
        create_private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o750);
    }

    #[test]
    fn restricts_existing_owned_directories() {
        let tmp = private_tempdir();
        let root = mkdir(&tmp.path().join("root"), 0o755);
        let net = mkdir(&root.join("regtest"), 0o775);
        let backups = mkdir(&net.join("backups"), 0o775);

        create_owned_dir(&root, Path::new("regtest/backups/auto")).unwrap();
        assert_eq!(mode(&net), 0o700);
        assert_eq!(
            mode(&backups),
            0o700,
            "every owned level, not just the last"
        );
        assert_eq!(mode(&backups.join("auto")), 0o700);
        assert_eq!(mode(&root), 0o755, "the parent is not the app's");

        // Only group and other bits go: 0555 becomes 0500, not 0700.
        chmod(&net, 0o555);
        create_owned_dir(&root, Path::new("regtest")).unwrap();
        assert_eq!(mode(&net), 0o500);
        chmod(&net, 0o700);
    }

    #[test]
    fn a_symlink_at_or_above_an_owned_directory_changes_nothing_outside() {
        let tmp = private_tempdir();
        let root = mkdir(&tmp.path().join("root"), 0o700);
        let outside = mkdir(&tmp.path().join("outside"), 0o775);
        let outside_backups = mkdir(&outside.join("backups"), 0o775);
        symlink(&outside, root.join("regtest")).unwrap();

        // The symlink is the owned path itself.
        create_owned_dir(&root, Path::new("regtest")).unwrap();
        assert_eq!(mode(&outside), 0o775);

        // The symlink is an intermediate owned component.
        create_owned_dir(&root, Path::new("regtest/backups/auto")).unwrap();
        assert_eq!(mode(&outside), 0o775);
        assert_eq!(mode(&outside_backups), 0o775);
        assert_eq!(
            mode(&outside_backups.join("auto")),
            0o700,
            "created, as mkdir -p would"
        );

        create_owned_file(&root, Path::new("regtest/app.sqlite")).unwrap();
        assert_eq!(
            mode(&outside.join("app.sqlite")),
            0o600,
            "created owner-only"
        );
        chmod(&outside.join("app.sqlite"), 0o664);
        create_owned_file(&root, Path::new("regtest/app.sqlite")).unwrap();
        assert_eq!(mode(&outside.join("app.sqlite")), 0o664);
    }

    #[test]
    fn a_symlink_in_the_chosen_part_is_followed() {
        let tmp = private_tempdir();
        let real = mkdir(&tmp.path().join("real"), 0o700);
        let net = mkdir(&real.join("regtest"), 0o775);
        symlink(&real, tmp.path().join("link")).unwrap();
        create_owned_dir(&tmp.path().join("link"), Path::new("regtest")).unwrap();
        assert_eq!(mode(&net), 0o700);

        // A relative target with `..`, as `ln -s ../real` makes.
        let sub = mkdir(&tmp.path().join("sub"), 0o700);
        symlink("../real", sub.join("rel")).unwrap();
        chmod(&net, 0o775);
        create_owned_dir(&sub.join("rel"), Path::new("regtest")).unwrap();
        assert_eq!(mode(&net), 0o700);
    }

    #[test]
    fn an_owned_directory_swapped_after_it_is_opened_is_the_one_changed() {
        let tmp = private_tempdir();
        // The walk reports resolved paths: /private/var/... on macOS.
        let base = tmp.path().canonicalize().unwrap();
        let root = mkdir(&base.join("root"), 0o700);
        let net = mkdir(&root.join("regtest"), 0o775);
        let outside = mkdir(&base.join("outside"), 0o775);
        let outside_backups = mkdir(&outside.join("backups"), 0o775);
        let moved = root.join("moved");
        {
            let (net, moved, outside) = (net.clone(), moved.clone(), outside.clone());
            on_open(net.clone(), move || {
                std::fs::rename(&net, &moved).unwrap();
                symlink(&outside, &net).unwrap();
            });
        }
        create_owned_dir(&root, Path::new("regtest/backups")).unwrap();

        assert_eq!(mode(&outside), 0o775, "the symlink target is not changed");
        assert_eq!(mode(&outside_backups), 0o775);
        assert_eq!(mode(&moved), 0o700, "the directory that was opened is");
        assert_eq!(
            mode(&moved.join("backups")),
            0o700,
            "the walk goes on below the opened directory"
        );
    }

    #[test]
    fn an_owned_file_swapped_after_it_is_opened_is_the_one_changed() {
        let tmp = private_tempdir();
        let base = tmp.path().canonicalize().unwrap();
        let root = mkdir(&base.join("root"), 0o700);
        let db = root.join("app.sqlite");
        std::fs::write(&db, b"kept").unwrap();
        chmod(&db, 0o664);
        let outside = base.join("outside.sqlite");
        std::fs::write(&outside, b"theirs").unwrap();
        chmod(&outside, 0o664);
        let moved = root.join("moved.sqlite");
        {
            let (db, moved, outside) = (db.clone(), moved.clone(), outside.clone());
            on_open(db.clone(), move || {
                std::fs::rename(&db, &moved).unwrap();
                symlink(&outside, &db).unwrap();
            });
        }
        create_owned_file(&root, Path::new("app.sqlite")).unwrap();
        assert_eq!(mode(&outside), 0o664);
        assert_eq!(mode(&moved), 0o600);
    }

    #[test]
    fn nothing_changes_below_a_directory_others_can_write() {
        let tmp = private_tempdir();
        let shared = mkdir(&tmp.path().join("shared"), 0o775);
        let root = mkdir(&shared.join("root"), 0o700);
        let net = mkdir(&root.join("regtest"), 0o775);
        create_owned_dir(&root, Path::new("regtest")).unwrap();
        assert_eq!(mode(&net), 0o775, "a group member could have swapped root");

        // With the sticky bit only the owner of an entry can replace it.
        chmod(&shared, 0o1777);
        create_owned_dir(&root, Path::new("regtest")).unwrap();
        assert_eq!(mode(&net), 0o700);
    }

    /// A parent the user may search but not read, as `/home` at 0711 on
    /// some distributions, is walked through (O_PATH).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_search_only_parent_is_no_obstacle() {
        let tmp = private_tempdir();
        let home = mkdir(&tmp.path().join("home"), 0o700);
        let root = mkdir(&home.join("root"), 0o700);
        let net = mkdir(&root.join("regtest"), 0o775);
        chmod(&home, 0o311);
        let result = create_owned_dir(&root, Path::new("regtest/backups"));
        chmod(&home, 0o700);
        result.unwrap();
        assert_eq!(mode(&net), 0o700);
        assert_eq!(mode(&net.join("backups")), 0o700);
    }

    #[test]
    fn creates_and_restricts_an_owned_file() {
        let tmp = private_tempdir();
        let db = tmp.path().join("app.sqlite");
        create_owned_file(tmp.path(), Path::new("app.sqlite")).unwrap();
        assert_eq!(mode(&db), 0o600);

        std::fs::write(&db, b"kept").unwrap();
        chmod(&db, 0o664);
        create_owned_file(tmp.path(), Path::new("app.sqlite")).unwrap();
        assert_eq!(mode(&db), 0o600);
        assert_eq!(
            std::fs::read(&db).unwrap(),
            b"kept",
            "contents are left alone"
        );

        // A second hard link would change elsewhere too.
        chmod(&db, 0o664);
        std::fs::hard_link(&db, tmp.path().join("other")).unwrap();
        create_owned_file(tmp.path(), Path::new("app.sqlite")).unwrap();
        assert_eq!(mode(&db), 0o664);
    }

    #[test]
    fn rejects_what_is_not_a_directory_or_a_plain_path() {
        let tmp = private_tempdir();
        let file = tmp.path().join("file");
        std::fs::write(&file, b"x").unwrap();
        assert!(create_private_dir(&file).is_err());
        assert!(create_private_dir(&file.join("below")).is_err());
        assert!(create_owned_dir(tmp.path(), Path::new("file")).is_err());
        assert!(create_owned_file(tmp.path(), Path::new("file/x")).is_err());
        assert!(create_owned_dir(tmp.path(), Path::new("../escape")).is_err());
        assert!(create_owned_dir(tmp.path(), Path::new("/abs")).is_err());
        mkdir(&tmp.path().join("dir"), 0o700);
        assert!(create_owned_file(tmp.path(), Path::new("dir")).is_err());
    }
}
