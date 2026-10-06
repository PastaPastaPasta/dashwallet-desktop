//! Log export for support requests (IOS-112): one zip with every log file
//! of the data root plus the files the host adds (its own log), and a
//! `manifest.txt` that lists what is in it and what could not be read.
//!
//! Collected from the data root: `<root>/logs/*` and
//! `<root>/<network>/logs/*` (regular files only, no recursion). The engine
//! does not write log files yet, so today the zip usually holds the host's
//! files and the manifest; the manifest says so rather than the zip being
//! padded with anything else. Logs never contain secrets (the engine's
//! secret-in-log test guards what it writes).

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::DesktopError;

/// What `export` wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogExport {
    pub path: PathBuf,
    /// Log files in the zip, not counting `manifest.txt`.
    pub file_count: u32,
    pub size_bytes: u64,
}

/// Largest single file copied into the zip; bigger ones are listed as
/// skipped (a runaway log should not make a 10 GB attachment).
pub const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// The log files under `data_root`, with their names inside the zip.
fn collect(data_root: &Path) -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    let mut add_dir = |dir: &Path, prefix: &str| {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .collect();
        files.sort();
        for path in files {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            found.push((path, format!("{prefix}{name}")));
        }
    };
    add_dir(&data_root.join("logs"), "logs/");
    if let Ok(entries) = std::fs::read_dir(data_root) {
        let mut networks: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n != "logs"))
            .collect();
        networks.sort();
        for net in networks {
            let name = net
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            add_dir(&net.join("logs"), &format!("{name}/logs/"));
        }
    }
    found
}

/// Writes the zip to `dest` (which must not exist; it is never replaced).
/// `extra_files` go under `app/`; a missing or unreadable one is listed in
/// the manifest as skipped. `version` names the build in the manifest.
pub fn export(
    data_root: &Path,
    dest: &Path,
    extra_files: &[PathBuf],
    version: &str,
) -> Result<LogExport, DesktopError> {
    if !dest.is_absolute() {
        return Err(DesktopError::InvalidArgument(format!(
            "destination {} is not absolute",
            dest.display()
        )));
    }
    let mut sources = collect(data_root);
    for (i, extra) in extra_files.iter().enumerate() {
        let name = extra
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("file-{i}"));
        sources.push((extra.clone(), format!("app/{name}")));
    }

    let file = OpenOptions::new().write(true).create_new(true).open(dest)?;
    let result = write_zip(file, &sources, data_root, version);
    match result {
        Ok(file_count) => Ok(LogExport {
            path: dest.to_path_buf(),
            file_count,
            size_bytes: std::fs::metadata(dest)?.len(),
        }),
        Err(e) => {
            // Leave no half-written zip behind.
            let _ = std::fs::remove_file(dest);
            Err(e)
        }
    }
}

fn write_zip(
    file: File,
    sources: &[(PathBuf, String)],
    data_root: &Path,
    version: &str,
) -> Result<u32, DesktopError> {
    let zip_error = |e: zip::result::ZipError| DesktopError::OsError(format!("zip: {e}"));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut zip = ZipWriter::new(file);
    let mut included = Vec::new();
    let mut skipped = Vec::new();
    let mut used_names = std::collections::HashSet::new();
    for (path, name) in sources {
        let mut name = name.clone();
        let mut n = 1;
        while !used_names.insert(name.clone()) {
            n += 1;
            name = format!("{name}.{n}");
        }
        let opened = File::open(path).and_then(|f| {
            let len = f.metadata()?.len();
            Ok((f, len))
        });
        match opened {
            Ok((_, len)) if len > MAX_FILE_BYTES => {
                skipped.push(format!("{} ({len} bytes, over the limit)", path.display()));
            }
            Ok((mut f, _)) => {
                zip.start_file(&name, options).map_err(zip_error)?;
                let mut buf = [0u8; 64 * 1024];
                loop {
                    let read = f.read(&mut buf)?;
                    if read == 0 {
                        break;
                    }
                    zip.write_all(&buf[..read])?;
                }
                included.push(name);
            }
            Err(e) => skipped.push(format!("{} ({e})", path.display())),
        }
    }
    let mut manifest = format!(
        "Dash Wallet log export\nversion: {version}\nos: {} {}\ndata directory: {}\n\nincluded ({}):\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
        data_root.display(),
        included.len()
    );
    for name in &included {
        manifest.push_str(&format!("  {name}\n"));
    }
    if included.is_empty() {
        manifest.push_str("  (no log files were found)\n");
    }
    if !skipped.is_empty() {
        manifest.push_str(&format!("\nskipped ({}):\n", skipped.len()));
        for s in &skipped {
            manifest.push_str(&format!("  {s}\n"));
        }
    }
    zip.start_file("manifest.txt", options).map_err(zip_error)?;
    zip.write_all(manifest.as_bytes())?;
    let mut file = zip.finish().map_err(zip_error)?;
    file.flush()?;
    file.sync_all()?;
    Ok(included.len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::ZipArchive;

    fn entries(path: &Path) -> Vec<(String, String)> {
        let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
        (0..archive.len())
            .map(|i| {
                let mut f = archive.by_index(i).unwrap();
                let mut text = String::new();
                f.read_to_string(&mut text).unwrap();
                (f.name().to_string(), text)
            })
            .collect()
    }

    #[test]
    fn test_IOS_112_zips_network_logs_and_host_files() {
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("testnet/logs")).unwrap();
        std::fs::create_dir_all(root.path().join("mainnet/logs")).unwrap();
        std::fs::create_dir_all(root.path().join("testnet/spv")).unwrap();
        std::fs::write(root.path().join("testnet/logs/engine.log"), "t1").unwrap();
        std::fs::write(root.path().join("mainnet/logs/engine.log"), "m1").unwrap();
        std::fs::write(root.path().join("testnet/spv/headers.dat"), "not a log").unwrap();
        let swift_log = out.path().join("app.log");
        std::fs::write(&swift_log, "swift").unwrap();
        let missing = out.path().join("gone.log");

        let dest = out.path().join("logs.zip");
        let report = export(root.path(), &dest, &[swift_log, missing], "0.1.0").unwrap();
        assert_eq!(report.file_count, 3);
        assert_eq!(report.size_bytes, std::fs::metadata(&dest).unwrap().len());

        let got = entries(&dest);
        let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "mainnet/logs/engine.log",
                "testnet/logs/engine.log",
                "app/app.log",
                "manifest.txt"
            ]
        );
        assert_eq!(got[0].1, "m1");
        assert_eq!(got[2].1, "swift");
        let manifest = &got[3].1;
        assert!(manifest.contains("version: 0.1.0"));
        assert!(manifest.contains("skipped (1):"));
        assert!(manifest.contains("gone.log"));

        // Never replaces a file.
        assert!(matches!(
            export(root.path(), &dest, &[], "0.1.0"),
            Err(DesktopError::OsError(_))
        ));
    }

    #[test]
    fn test_IOS_112_an_empty_root_still_gives_a_manifest() {
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("logs.zip");
        let report = export(root.path(), &dest, &[], "0.1.0").unwrap();
        assert_eq!(report.file_count, 0);
        let got = entries(&dest);
        assert_eq!(got.len(), 1);
        assert!(got[0].1.contains("(no log files were found)"));
        assert!(matches!(
            export(root.path(), Path::new("relative.zip"), &[], "x"),
            Err(DesktopError::InvalidArgument(_))
        ));
    }
}
