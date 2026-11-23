//! Writing freedesktop.org files: `.desktop` entries (Desktop Entry
//! Specification 1.5) and `mimeapps.list` defaults (MIME Applications
//! Associations 1.0.1). Used by autostart and URI registration on Linux;
//! pure text functions, tested on every OS.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::path::{Path, PathBuf};

use crate::DesktopError;

/// One argument of an `Exec` key. Arguments with reserved characters are
/// double-quoted with `"`, `` ` ``, `$` and `\` backslash-escaped; `%` is
/// doubled in every argument (field codes are added by the caller).
pub(crate) fn exec_arg(arg: &str) -> String {
    let escaped = arg.replace('%', "%%");
    let reserved = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\''
                    | '\\'
                    | '>'
                    | '<'
                    | '~'
                    | '|'
                    | '&'
                    | ';'
                    | '$'
                    | '*'
                    | '?'
                    | '#'
                    | '('
                    | ')'
                    | '`'
            )
    };
    if !escaped.is_empty() && !escaped.chars().any(reserved) {
        return escaped;
    }
    let mut out = String::with_capacity(escaped.len() + 2);
    out.push('"');
    for c in escaped.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The `Exec` value for `args` (program first): each argument quoted by
/// [`exec_arg`], then the line escaped as a string value, so a backslash
/// becomes `\\\\` inside quotes and a line break cannot start a new key.
pub(crate) fn exec_line<'a>(args: impl IntoIterator<Item = &'a str>) -> String {
    let quoted: Vec<String> = args.into_iter().map(exec_arg).collect();
    value(&quoted.join(" "))
}

/// A value of a string key: no line breaks (`\n`, `\r` escaped per spec)
/// and backslashes escaped.
pub(crate) fn value(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// `$XDG_CONFIG_HOME` (absolute) else `~/.config`.
pub(crate) fn config_home() -> Result<PathBuf, DesktopError> {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

/// `$XDG_DATA_HOME` (absolute) else `~/.local/share`.
pub(crate) fn data_home() -> Result<PathBuf, DesktopError> {
    xdg_dir("XDG_DATA_HOME", ".local/share")
}

fn xdg_dir(var: &str, fallback: &str) -> Result<PathBuf, DesktopError> {
    if let Some(dir) = std::env::var_os(var).map(PathBuf::from)
        && dir.is_absolute()
    {
        return Ok(dir);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
        .ok_or_else(|| DesktopError::OsError("HOME is not set".into()))?;
    Ok(home.join(fallback))
}

/// Writes `contents` to `path` atomically (temp file + rename), creating
/// the parent directory. Missing directories are created 0700, as the XDG
/// Base Directory spec asks: a `~/.local/share` created with the umask's
/// 0775 (umask 002 on Ubuntu and Fedora desktops) would make the wallet
/// storage refuse the data root below it.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<(), DesktopError> {
    let parent = path
        .parent()
        .ok_or_else(|| DesktopError::OsError(format!("{} has no parent", path.display())))?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)?;
    let tmp = path.with_extension("tmp-dw");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// `mimeapps.list` with `handler` as the default for each MIME type in
/// `mime_types`. Other lines, groups and comments are kept; existing
/// entries for those types are replaced.
pub(crate) fn set_mime_defaults(existing: &str, mime_types: &[String], handler: &str) -> String {
    const GROUP: &str = "[Default Applications]";
    let mut out: Vec<String> = Vec::new();
    let mut in_group = false;
    let mut saw_group = false;
    let mut inserted = false;
    let ours = |line: &str| {
        line.split_once('=')
            .is_some_and(|(k, _)| mime_types.iter().any(|m| m == k.trim()))
    };
    let entries: Vec<String> = mime_types
        .iter()
        .map(|m| format!("{m}={handler};"))
        .collect();
    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_group && !inserted {
                out.extend(entries.iter().cloned());
                inserted = true;
            }
            in_group = trimmed == GROUP;
            saw_group |= in_group;
            out.push(line.to_string());
            continue;
        }
        if in_group && ours(trimmed) {
            continue;
        }
        out.push(line.to_string());
    }
    if in_group && !inserted {
        out.extend(entries.iter().cloned());
        inserted = true;
    }
    if !saw_group {
        if out.last().is_some_and(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push(GROUP.to_string());
        out.extend(entries);
    } else {
        debug_assert!(inserted);
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_arguments_are_quoted_per_spec() {
        assert_eq!(exec_arg("/opt/dash/dash-wallet"), "/opt/dash/dash-wallet");
        assert_eq!(exec_arg("--min"), "--min");
        assert_eq!(
            exec_arg("/home/a b/Dash Wallet"),
            "\"/home/a b/Dash Wallet\""
        );
        assert_eq!(exec_arg("a\"b$c`d\\e"), "\"a\\\"b\\$c\\`d\\\\e\"");
        // The value escape doubles every backslash of the quoted form.
        assert_eq!(exec_line(["/a b", "x\\y"]), "\"/a b\" \"x\\\\\\\\y\"");
        assert_eq!(exec_line(["/x\nName=evil"]), "\"/x\\nName=evil\"");
        assert_eq!(exec_arg("100%"), "100%%");
        assert_eq!(exec_arg(""), "\"\"");
        assert_eq!(value("a\nb\\c"), "a\\nb\\\\c");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_creates_missing_directories_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".local/share/applications/x.desktop");
        write_atomic(&path, "[Desktop Entry]\n").unwrap();
        for dir in [".local", ".local/share", ".local/share/applications"] {
            let mode = std::fs::metadata(tmp.path().join(dir))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "{dir}");
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[Desktop Entry]\n");
    }

    #[test]
    fn mime_defaults_replace_only_our_entries() {
        let types = vec![
            "x-scheme-handler/dash".to_string(),
            "x-scheme-handler/pay".into(),
        ];
        let fresh = set_mime_defaults("", &types, "dashwallet.desktop");
        assert_eq!(
            fresh,
            "[Default Applications]\nx-scheme-handler/dash=dashwallet.desktop;\nx-scheme-handler/pay=dashwallet.desktop;\n"
        );

        let existing = "# mine\n[Added Associations]\ntext/plain=gedit.desktop;\n\n[Default Applications]\nx-scheme-handler/dash=other.desktop;\ntext/html=firefox.desktop;\n";
        let merged = set_mime_defaults(existing, &types, "dashwallet.desktop");
        assert_eq!(
            merged,
            "# mine\n[Added Associations]\ntext/plain=gedit.desktop;\n\n[Default Applications]\ntext/html=firefox.desktop;\nx-scheme-handler/dash=dashwallet.desktop;\nx-scheme-handler/pay=dashwallet.desktop;\n"
        );
        // Idempotent.
        assert_eq!(
            set_mime_defaults(&merged, &types, "dashwallet.desktop"),
            merged
        );

        let other_group_only = "[Added Associations]\ntext/plain=gedit.desktop;";
        assert_eq!(
            set_mime_defaults(other_group_only, &types[..1], "d.desktop"),
            "[Added Associations]\ntext/plain=gedit.desktop;\n\n[Default Applications]\nx-scheme-handler/dash=d.desktop;\n"
        );
    }
}
