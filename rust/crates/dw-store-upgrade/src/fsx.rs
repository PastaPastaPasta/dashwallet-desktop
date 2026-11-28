//! File-system helpers: private directories, durable writes, UTC stamps and
//! the test-only fault points.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    fs::DirBuilder::new().create(path)
}

pub(crate) fn check_private_dir(path: &Path) -> io::Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let safe = meta.is_dir();
    #[cfg(unix)]
    let safe = {
        use std::os::unix::fs::MetadataExt;
        safe && meta.uid() == rustix::process::geteuid().as_raw() && meta.mode() & 0o022 == 0
    };
    if safe {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not an owner-controlled directory", path.display()),
        ))
    }
}

pub(crate) fn ensure_private_dir(path: &Path) -> io::Result<()> {
    match create_private_dir(path) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
        other => other,
    }
}

pub(crate) fn fsync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

pub(crate) fn fsync_file(path: &Path) -> io::Result<()> {
    OpenOptions::new()
        .read(true)
        .write(!cfg!(unix))
        .open(path)?
        .sync_all()
}

/// Write `bytes` to `path` via `path.partial`, fsync, rename, fsync the dir.
pub(crate) fn write_durable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".partial");
    let tmp = std::path::PathBuf::from(tmp);
    let written = (|| {
        let mut opts = OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, path)?;
    fsync_dir(dir)
}

/// Remove `path` if present and fsync its directory.
pub(crate) fn remove_durable(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    }
    fsync_dir(path.parent().unwrap_or(Path::new(".")))
}

/// `YYYYMMDDTHHMMSSZ` for `now`, in UTC.
pub(crate) fn utc_stamp(now: SystemTime) -> String {
    let secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// A directory name `{prefix}-{stamp}`, suffixed `-2`, `-3`… when taken.
pub(crate) fn fresh_name(parent: &Path, prefix: &str, now: SystemTime) -> String {
    let base = format!("{prefix}-{}", utc_stamp(now));
    if !parent.join(&base).exists() && !parent.join(format!("{base}.staging")).exists() {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|name| !parent.join(name).exists())
        .unwrap_or(base)
}

#[cfg(test)]
thread_local! {
    static FAULT: std::cell::RefCell<Option<(&'static str, u32)>> =
        const { std::cell::RefCell::new(None) };
}

/// Arm a one-shot fault at the `skip`-th hit (0-based) of `point`.
#[cfg(test)]
pub(crate) fn arm_fault(point: &'static str, skip: u32) {
    FAULT.with(|f| *f.borrow_mut() = Some((point, skip)));
}

#[cfg(test)]
pub(crate) fn disarm_fault() {
    FAULT.with(|f| *f.borrow_mut() = None);
}

#[cfg(test)]
pub(crate) fn fault(point: &'static str) -> crate::Result<()> {
    FAULT.with(|f| {
        let mut f = f.borrow_mut();
        match *f {
            Some((p, 0)) if p == point => {
                *f = None;
                Err(crate::StoreUpgradeError::InjectedFault(point))
            }
            Some((p, n)) if p == point => {
                *f = Some((p, n - 1));
                Ok(())
            }
            _ => Ok(()),
        }
    })
}

#[cfg(not(test))]
#[inline(always)]
pub(crate) fn fault(_point: &'static str) -> crate::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn utc_stamp_formats_known_instants() {
        assert_eq!(utc_stamp(UNIX_EPOCH), "19700101T000000Z");
        let t = UNIX_EPOCH + Duration::from_secs(1_795_567_800);
        assert_eq!(utc_stamp(t), "20261125T005000Z");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(utc_stamp(leap), "20000229T000000Z");
    }
}
