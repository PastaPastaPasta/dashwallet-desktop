//! The avatar thumbnail cache (DASHPAY §3.4, §3.8; DP4-02 design, "The
//! cache"): files in `<network>/avatars/`, reached through one directory
//! descriptor opened once per session, and their index in `dp_avatar`.
//! A file is created `O_CREAT | O_EXCL | O_NOFOLLOW` under a temporary name
//! and renamed into place, read `O_NOFOLLOW` after an `fstat`, and unlinked
//! by name in the directory: never by a path that a swapped-in symlink
//! could redirect. Not part of the facade.

use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use dw_appdb::{AppDb, AvatarRow};
use sha2::{Digest, Sha256};

use super::avatar_image::{Crop, Ingested};
use super::profile::AvatarSize;

/// Rotation (§3.4): the thumbnails' bytes, and the rows.
const MAX_CACHE_BYTES: u64 = 200_000_000;
const MAX_ROWS: u64 = 10_000;
/// The largest thumbnail file read back.
const MAX_FILE: u64 = 1 << 20;
/// A temporary file's name prefix; leftovers are removed at open.
const TMP_PREFIX: &str = ".tmp-";
/// Thumbnail pairs kept in memory when there is no directory (D10).
const IN_MEMORY: usize = 64;

/// The stem of a picture's thumbnail files: the content and the crop
/// (D15), so two crops of one picture never share a file.
pub(super) fn stem(content_sha: &[u8; 32], crop: Option<&Crop>) -> String {
    let mut h = Sha256::new();
    h.update(b"dw-avatar-thumb-v1\0");
    h.update(content_sha);
    if let Some(crop) = crop {
        h.update(crop.key().as_bytes());
    }
    hex::encode(h.finalize())
}

fn file_name(stem: &str, size: AvatarSize) -> String {
    format!("{stem}-{}.png", size.px())
}

/// A stem the cache wrote: 64 lower-case hex digits, so no name built from
/// it can leave the directory.
fn valid_stem(stem: &str) -> bool {
    stem.len() == 64 && stem.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The avatars directory, by descriptor (Unix) or path (Windows, D16).
pub(super) struct ThumbDir {
    #[cfg(unix)]
    fd: std::os::fd::OwnedFd,
    #[cfg(not(unix))]
    path: std::path::PathBuf,
}

#[cfg(unix)]
impl ThumbDir {
    /// Opens the directory once. A user's symlink standing in for it is
    /// followed (§3.8), but the directory reached must be the user's and
    /// writable by nobody else (D10). Leftover temporary files go.
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        use rustix::fs::{Mode, OFlags};
        let fd = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let st = rustix::fs::fstat(&fd)?;
        let refused = |why: String| io::Error::new(io::ErrorKind::PermissionDenied, why);
        if st.st_uid != rustix::process::geteuid().as_raw() {
            return Err(refused(
                "the avatars directory is not the current user's".into(),
            ));
        }
        if (st.st_mode as u32) & 0o022 != 0 {
            return Err(refused(format!(
                "the avatars directory has mode {:o}: group or others may write it",
                (st.st_mode as u32) & 0o7777
            )));
        }
        let dir = Self { fd };
        dir.remove_temporaries();
        Ok(dir)
    }

    fn remove_temporaries(&self) {
        let Ok(entries) = rustix::fs::Dir::read_from(&self.fd) else {
            return;
        };
        for entry in entries.flatten() {
            if let Ok(name) = entry.file_name().to_str()
                && name.starts_with(TMP_PREFIX)
            {
                let _ = rustix::fs::unlinkat(&self.fd, name, rustix::fs::AtFlags::empty());
            }
        }
    }

    /// Writes `name` whole: a new temporary file (`O_EXCL | O_NOFOLLOW`,
    /// 0600), synced, then renamed over `name`.
    fn write(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        use rustix::fs::{Mode, OFlags};
        use std::io::Write as _;
        let tmp = format!("{TMP_PREFIX}{:016x}", rand::random::<u64>());
        let fd = rustix::fs::openat(
            &self.fd,
            tmp.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?;
        let mut file = std::fs::File::from(fd);
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        let renamed = written.and_then(|()| {
            Ok(rustix::fs::renameat(
                &self.fd,
                tmp.as_str(),
                &self.fd,
                name,
            )?)
        });
        if renamed.is_err() {
            let _ = rustix::fs::unlinkat(&self.fd, tmp.as_str(), rustix::fs::AtFlags::empty());
        }
        renamed
    }

    /// `name`'s bytes, `None` when it is missing; a symlink, anything but a
    /// regular file, or a file over [`MAX_FILE`] is an error.
    fn read(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        use rustix::fs::{FileType, Mode, OFlags};
        use std::io::Read as _;
        let fd = match rustix::fs::openat(
            &self.fd,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let st = rustix::fs::fstat(&fd)?;
        if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile
            || st.st_size as u64 > MAX_FILE
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a thumbnail file",
            ));
        }
        let mut out = Vec::with_capacity(st.st_size as usize);
        std::fs::File::from(fd)
            .take(MAX_FILE)
            .read_to_end(&mut out)?;
        Ok(Some(out))
    }

    fn remove(&self, name: &str) {
        let _ = rustix::fs::unlinkat(&self.fd, name, rustix::fs::AtFlags::empty());
    }
}

#[cfg(not(unix))]
impl ThumbDir {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        if !std::fs::metadata(path)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a directory",
            ));
        }
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    fn write(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write as _;
        let tmp = self
            .path
            .join(format!("{TMP_PREFIX}{:016x}", rand::random::<u64>()));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp, self.path.join(name))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    fn read(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        let path = self.path.join(name);
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
            Ok(m) if !m.is_file() || m.len() > MAX_FILE => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a thumbnail file",
            )),
            Ok(_) => std::fs::read(path).map(Some),
        }
    }

    fn remove(&self, name: &str) {
        let _ = std::fs::remove_file(self.path.join(name));
    }
}

/// The session's thumbnails and their index.
pub(super) struct AvatarStore {
    dir: Option<ThumbDir>,
    /// Without a directory: the latest thumbnail pairs by stem, so a
    /// verified picture is not fetched again on every read.
    memory: Mutex<VecDeque<(String, [Vec<u8>; 2])>>,
    db: Arc<AppDb>,
    /// Writes and rotation one at a time, so a file is never unlinked
    /// between another write's row and its check.
    writes: Mutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl AvatarStore {
    pub(super) fn new(dir: Option<ThumbDir>, db: Arc<AppDb>) -> Self {
        Self {
            dir,
            memory: Mutex::default(),
            db,
            writes: Mutex::new(()),
        }
    }

    pub(super) fn row(&self, url_sha: &str) -> Option<AvatarRow> {
        (self.db.avatar_row(url_sha))
            .inspect_err(|e| tracing::warn!(error = %e, "avatar index read failed"))
            .ok()
            .flatten()
    }

    /// The thumbnail `row` names at `size`, if it is still on disk and is
    /// a PNG of that size (D10).
    pub(super) fn thumbnail(&self, row: &AvatarRow, size: AvatarSize) -> Option<Vec<u8>> {
        let stem = row.file.as_deref().filter(|s| valid_stem(s))?;
        let Some(dir) = &self.dir else {
            let memory = lock(&self.memory);
            let (_, pair) = memory.iter().find(|(s, _)| s == stem)?;
            return Some(pair[usize::from(size == AvatarSize::Large)].clone());
        };
        let png = (dir.read(&file_name(stem, size)))
            .inspect_err(|e| tracing::warn!(error = %e, "avatar thumbnail unreadable"))
            .ok()??;
        super::avatar_image::is_thumbnail(&png, size.px()).then_some(png)
    }

    /// Records a fetch that decoded: the thumbnails written (to the
    /// directory, else memory), the row, then the rotation. A pair written
    /// halfway is removed, and the row then names no file.
    pub(super) fn record_ok(&self, url_sha: &str, stem: &str, ingested: &Ingested, now: u64) {
        let _w = lock(&self.writes);
        let sizes = [AvatarSize::Small, AvatarSize::Large];
        let (file, bytes) = match &self.dir {
            None => {
                let mut memory = lock(&self.memory);
                memory.retain(|(s, _)| s != stem);
                if memory.len() >= IN_MEMORY {
                    memory.pop_front();
                }
                memory.push_back((
                    stem.to_owned(),
                    sizes.map(|s| ingested.thumbnail(s).to_vec()),
                ));
                (Some(stem.to_owned()), None)
            }
            Some(dir) => {
                let written = sizes.iter().try_fold(0u64, |total, &size| {
                    let png = ingested.thumbnail(size);
                    dir.write(&file_name(stem, size), png)
                        .map(|()| total + png.len() as u64)
                });
                match written {
                    Ok(total) => (Some(stem.to_owned()), Some(total)),
                    Err(e) => {
                        tracing::warn!(error = %e, "avatar thumbnail not written");
                        sizes
                            .iter()
                            .for_each(|&size| dir.remove(&file_name(stem, size)));
                        (None, None)
                    }
                }
            }
        };
        self.put(&AvatarRow {
            url_sha: url_sha.to_owned(),
            content_sha: Some(hex::encode(ingested.sha)),
            dhash: ingested.dhash.map(hex::encode),
            status: "ok".into(),
            file,
            fetched_at: now,
            bytes,
        });
    }

    /// Records a fetch or decode that failed with `status`.
    pub(super) fn record_failure(&self, url_sha: &str, status: &str, now: u64) {
        let _w = lock(&self.writes);
        self.put(&AvatarRow {
            url_sha: url_sha.to_owned(),
            content_sha: None,
            dhash: None,
            status: status.to_owned(),
            file: None,
            fetched_at: now,
            bytes: None,
        });
    }

    fn put(&self, row: &AvatarRow) {
        let mut orphans = Vec::new();
        match self.db.put_avatar_row(row) {
            Ok(orphan) => orphans.extend(orphan),
            Err(e) => tracing::warn!(error = %e, "avatar index write failed"),
        }
        match self.db.rotate_avatars(MAX_CACHE_BYTES, MAX_ROWS) {
            Ok(rotated) => orphans.extend(rotated),
            Err(e) => tracing::warn!(error = %e, "avatar cache rotation failed"),
        }
        if let Some(dir) = &self.dir {
            for stem in orphans.iter().filter(|s| valid_stem(s)) {
                for size in [AvatarSize::Small, AvatarSize::Large] {
                    dir.remove(&file_name(stem, size));
                }
            }
        }
    }
}
