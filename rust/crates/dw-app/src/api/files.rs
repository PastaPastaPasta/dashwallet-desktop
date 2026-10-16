//! Files the user picks in the OS's own dialogs (U-05 D2): a PSBT to load
//! or save (QT-077, QT-078), the address book's export (QT-095), and the
//! paths U-04's backups, exports and imports take. The dialog runs in the
//! shell (`shell::files`); the renderer never names a path, it only gets
//! back the one the user chose.

use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppCode, AppError};
use crate::ipc::commands;
use crate::shell::files::{FilePurpose, FileRequest};
use crate::state::AppState;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

/// A file the user opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OpenedFile {
    /// The file's name, without its directory.
    pub name: String,
    /// Its bytes, base64.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::base64")]
    pub data: Vec<u8>,
}

commands! {
    /// Shows the open dialog for `purpose` and reads the chosen file; `null`
    /// when the user cancelled. `app.file_too_large` at or above the
    /// purpose's limit (a PSBT: 100 MiB, dash-qt's), `app.file_read` when it
    /// cannot be read.
    #[facade()]
    fn open_file(state, purpose: FilePurpose) -> Option<OpenedFile> {
        open(&state, purpose).await
    }

    /// Shows the save dialog for `purpose`, offering `suggested_name`, and
    /// writes `data` (base64) there, readable by the user only (0600 on
    /// Unix). A file already there is replaced only after the OS's own
    /// question, and whole: the bytes go to a new file that then takes its
    /// place. Returns the path, `null` when the user cancelled.
    #[facade()]
    fn save_file_as(state, purpose: FilePurpose, suggested_name: String, data: String) -> Option<String> {
        use ::base64::Engine as _;
        let data = ::base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| AppError::invalid_argument(format!("data: {e}")))?;
        let Some(path) = choose(&state, purpose, true, Some(suggested_name)).await? else {
            return Ok(None);
        };
        let text = utf8(path.clone())?;
        tauri::async_runtime::spawn_blocking(move || write_owner_only(&path, &data))
            .await
            .map_err(|e| AppError::app(AppCode::FileWrite, e.to_string()))?
            .map_err(|e| AppError::app(AppCode::FileWrite, e.to_string()))?;
        Ok(Some(text))
    }

    /// The dialog alone, for a file the engine reads or writes itself (U-04's
    /// backups, exports and imports): the path the user chose, `null` when
    /// they cancelled. A save dialog asks before naming an existing file.
    #[facade()]
    fn choose_file_path(state, purpose: FilePurpose, save: bool, suggested_name: Option<String>) -> Option<String> {
        choose(&state, purpose, save, suggested_name).await?.map(utf8).transpose()
    }
}

/// `open_file`'s dialog and read, for the commands that read a file the
/// user picks (`scan_qr_file`).
pub(crate) async fn open(state: &AppState, purpose: FilePurpose) -> Result<Option<OpenedFile>, AppError> {
    let Some(path) = choose(state, purpose, false, None).await? else {
        return Ok(None);
    };
    let max = purpose.spec().max_bytes;
    let data = tauri::async_runtime::spawn_blocking(move || read_capped(&path, max).map(|d| (path, d)))
        .await
        .map_err(|e| AppError::app(AppCode::FileRead, e.to_string()))?;
    let (path, data) = data?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Some(OpenedFile { name, data }))
}

async fn choose(
    state: &AppState,
    purpose: FilePurpose,
    save: bool,
    suggested_name: Option<String>,
) -> Result<Option<PathBuf>, AppError> {
    let request = FileRequest {
        purpose,
        save,
        suggested_name: suggested_name.and_then(|n| bare_file_name(&n)),
    };
    state
        .shell
        .choose(request)
        .await
        .map_err(|e| AppError::app(AppCode::FileDialog, e))
}

/// The renderer's suggestion as a bare file name: no directory part (either
/// separator), no control characters, at most 200 characters; `None` when
/// nothing is left. The dialog then opens where the OS would, never where a
/// page chose.
fn bare_file_name(name: &str) -> Option<String> {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = last.chars().filter(|c| !c.is_control()).take(200).collect();
    let clean = clean.trim();
    (!clean.is_empty() && clean != "." && clean != "..").then(|| clean.to_owned())
}

/// The engine's paths are strings: a path that is not UTF-8 cannot reach it.
fn utf8(path: PathBuf) -> Result<String, AppError> {
    path.into_os_string()
        .into_string()
        .map_err(|_| AppError::app(AppCode::FileDialog, "the chosen path is not valid UTF-8"))
}

/// The regular file at `path`, refused at `max` bytes or more before and
/// while it is read.
fn read_capped(path: &Path, max: u64) -> Result<Vec<u8>, AppError> {
    let read_error = |e: io::Error| AppError::app(AppCode::FileRead, e.to_string());
    let too_large = || {
        AppError::app(
            AppCode::FileTooLarge,
            format!("the file must be smaller than {} MiB", max / (1024 * 1024)),
        )
    };
    let not_regular = || AppError::app(AppCode::FileRead, "not a regular file");
    // Checked before the open too: opening a FIFO for reading would wait for a writer.
    if !std::fs::metadata(path).map_err(read_error)?.is_file() {
        return Err(not_regular());
    }
    let file = std::fs::File::open(path).map_err(read_error)?;
    let meta = file.metadata().map_err(read_error)?;
    if !meta.is_file() {
        return Err(not_regular());
    }
    if meta.len() >= max {
        return Err(too_large());
    }
    let mut data = Vec::with_capacity(meta.len() as usize);
    file.take(max).read_to_end(&mut data).map_err(read_error)?;
    if data.len() as u64 >= max {
        return Err(too_large());
    }
    Ok(data)
}

/// Writes `data` to a new owner-only file beside `path` and renames it onto
/// `path`: a reader never sees half the bytes, and a file that was there is
/// replaced whole, with the new file's mode.
fn write_owner_only(path: &Path, data: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let (part, mut file) = (0..100)
        .find_map(|n| {
            let mut part_name = std::ffi::OsString::from(".");
            part_name.push(name);
            part_name.push(format!(".{}-{n}.part", std::process::id()));
            let part = dir.join(part_name);
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&part) {
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => None,
                opened => Some(opened.map(|f| (part, f))),
            }
        })
        .unwrap_or_else(|| {
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "no free temporary name",
            ))
        })?;
    let written = file
        .write_all(data)
        .and_then(|()| file.sync_all())
        .and_then(|()| std::fs::rename(&part, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_read_below_their_limit_only() {
        let dir = dw_testutil::private_tempdir();
        let path = dir.path().join("a.psbt");
        std::fs::write(&path, [7u8; 10]).unwrap();
        assert_eq!(read_capped(&path, 11).unwrap(), [7u8; 10]);
        let e = read_capped(&path, 10).unwrap_err();
        assert_eq!(e.code_str(), AppCode::FileTooLarge.as_str());
        assert!(e.to_string().contains("smaller than"), "{e}");
        let e = read_capped(dir.path(), 11).unwrap_err();
        assert_eq!(e.code_str(), AppCode::FileRead.as_str());
        let e = read_capped(&dir.path().join("missing"), 11).unwrap_err();
        assert_eq!(e.code_str(), AppCode::FileRead.as_str());
    }

    #[test]
    fn a_suggested_name_is_a_bare_file_name() {
        assert_eq!(bare_file_name("yPay-1.5 tDASH.psbt").as_deref(), Some("yPay-1.5 tDASH.psbt"));
        assert_eq!(bare_file_name("../../.bashrc").as_deref(), Some(".bashrc"));
        assert_eq!(bare_file_name("C:\\Users\\x\\a.csv").as_deref(), Some("a.csv"));
        assert_eq!(bare_file_name("a\nb\u{7}.csv").as_deref(), Some("ab.csv"));
        assert_eq!(bare_file_name("dir/").as_deref(), None);
        assert_eq!(bare_file_name("..").as_deref(), None);
        assert_eq!(bare_file_name(&"x".repeat(300)).map(|n| n.len()), Some(200));
    }

    #[test]
    fn the_psbt_limit_is_dash_qts() {
        assert_eq!(FilePurpose::Psbt.spec().max_bytes, 100 * 1024 * 1024);
    }

    #[test]
    fn saved_files_are_the_users_alone_and_replace_whole() {
        let dir = dw_testutil::private_tempdir();
        let path = dir.path().join("tx.psbt");
        write_owner_only(&path, b"one").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            write_owner_only(&path, b"two").unwrap();
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        #[cfg(not(unix))]
        write_owner_only(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        // No temporary file stays behind.
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["tx.psbt"]);
        // A directory in the way is an error, and leaves nothing either.
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        assert!(write_owner_only(&dir.path().join("sub"), b"x").is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
