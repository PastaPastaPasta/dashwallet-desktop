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
/// the parent directory.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<(), DesktopError> {
    let parent = path
        .parent()
        .ok_or_else(|| DesktopError::OsError(format!("{} has no parent", path.display())))?;
    std::fs::create_dir_all(parent)?;
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
        assert_eq!(exec_arg("100%"), "100%%");
        assert_eq!(exec_arg(""), "\"\"");
        assert_eq!(value("a\nb\\c"), "a\\nb\\\\c");
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
