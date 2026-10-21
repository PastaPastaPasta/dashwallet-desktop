//! The renderer's display settings (U-04 D2): `<data root>/settings.json`,
//! next to `window-state.json`. One file for the data root, not per network.
//! A missing or unreadable file reads as the defaults; a write replaces the
//! file atomically (a temporary file renamed over it). Nothing secret is
//! kept here.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::api::units::DisplayUnit;
use crate::dto::ThemeName;
use crate::error::{AppError, EngineCode};

/// The settings file's name in the data root.
pub const FILE: &str = "settings.json";

/// Decimal digits of Home's balances (UX-SPEC §5.2, QT-020).
pub const DIGITS: std::ops::RangeInclusive<u8> = 2..=8;

/// The longest auto-lock wait: 24 hours (IOS-015).
pub const MAX_AUTO_LOCK_SECS: u32 = 86_400;

/// The auto-lock waits a file may hold, in seconds.
const AUTO_LOCK_SECS: std::ops::RangeInclusive<u32> = 1..=MAX_AUTO_LOCK_SECS;

/// What Settings ▸ General and Security keep for the renderer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct UiSettings {
    /// The appearance; `null` follows the system. `--theme` wins over it.
    pub theme: Option<ThemeName>,
    /// The unit amounts are shown and typed in (QT-139).
    pub unit: DisplayUnit,
    /// Decimal digits of Home's balances, 2..=8.
    pub decimal_digits: u8,
    /// Discreet mode is on when the app starts (IOS-020).
    pub autohide_balance: bool,
    /// Seconds without input before the vault locks itself; `null`: never
    /// (IOS-015). 1..=86400.
    pub auto_lock_secs: Option<u32>,
    /// A new payment starts with "Subtract fee from amount" checked (QT-137).
    pub subtract_fee_by_default: bool,
    /// Settings ▸ General ▸ Show Masternodes: the sidebar item shows even
    /// without a masternode, a collateral or a proposal (MG-00 D1, D2).
    pub show_masternodes: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        UiSettings {
            theme: None,
            unit: DisplayUnit::Dash,
            decimal_digits: 2,
            autohide_balance: false,
            auto_lock_secs: None,
            subtract_fee_by_default: false,
            show_masternodes: false,
        }
    }
}

impl UiSettings {
    /// Refuses values the renderer must not store.
    pub fn check(&self) -> Result<(), AppError> {
        if !DIGITS.contains(&self.decimal_digits) {
            return Err(AppError::invalid_argument("decimal digits must be 2 to 8"));
        }
        if let Some(secs) = self.auto_lock_secs
            && !AUTO_LOCK_SECS.contains(&secs)
        {
            return Err(AppError::invalid_argument(
                "auto lock must be 1 s to 24 hours",
            ));
        }
        Ok(())
    }

    /// A file edited by hand may hold anything: out-of-range values read as
    /// their defaults.
    fn sanitized(mut self) -> Self {
        let defaults = UiSettings::default();
        if !DIGITS.contains(&self.decimal_digits) {
            self.decimal_digits = defaults.decimal_digits;
        }
        if self
            .auto_lock_secs
            .is_some_and(|s| !AUTO_LOCK_SECS.contains(&s))
        {
            self.auto_lock_secs = defaults.auto_lock_secs;
        }
        self
    }
}

pub fn path(root: &Path) -> PathBuf {
    root.join(FILE)
}

/// The settings in `root`; the defaults when there is no readable file.
pub fn read(root: &Path) -> UiSettings {
    let Ok(text) = std::fs::read_to_string(path(root)) else {
        return UiSettings::default();
    };
    match serde_json::from_str::<UiSettings>(&text) {
        Ok(s) => s.sanitized(),
        Err(e) => {
            eprintln!("dw-app: {FILE} is unreadable, using the defaults: {e}");
            UiSettings::default()
        }
    }
}

/// Checks and writes `settings` into `root`, replacing the file atomically.
pub fn write(root: &Path, settings: &UiSettings) -> Result<(), AppError> {
    settings.check()?;
    let io = |e: std::io::Error| AppError::new(EngineCode::Io, format!("cannot save {FILE}: {e}"));
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| AppError::new(EngineCode::Internal, e.to_string()))?;
    let tmp = root.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, json).map_err(io)?;
    std::fs::rename(&tmp, path(root)).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn a_missing_file_reads_as_the_defaults() {
        let dir = root();
        assert_eq!(read(dir.path()), UiSettings::default());
    }

    #[test]
    fn a_written_file_reads_back() {
        let dir = root();
        let s = UiSettings {
            theme: Some(ThemeName::Dark),
            unit: DisplayUnit::MilliDash,
            decimal_digits: 8,
            autohide_balance: true,
            auto_lock_secs: Some(300),
            subtract_fee_by_default: true,
            show_masternodes: true,
        };
        write(dir.path(), &s).unwrap();
        assert_eq!(read(dir.path()), s);
        assert!(!dir.path().join("settings.json.tmp").exists());
    }

    #[test]
    fn out_of_range_values_are_refused() {
        let dir = root();
        for bad in [
            UiSettings {
                decimal_digits: 1,
                ..Default::default()
            },
            UiSettings {
                decimal_digits: 9,
                ..Default::default()
            },
            UiSettings {
                auto_lock_secs: Some(0),
                ..Default::default()
            },
            UiSettings {
                auto_lock_secs: Some(MAX_AUTO_LOCK_SECS + 1),
                ..Default::default()
            },
        ] {
            let e = write(dir.path(), &bad).unwrap_err();
            assert_eq!(e.code_str(), "invalid_argument");
        }
        assert!(!path(dir.path()).exists());
    }

    #[test]
    fn an_unreadable_or_hand_edited_file_falls_back() {
        let dir = root();
        std::fs::write(path(dir.path()), "{not json").unwrap();
        assert_eq!(read(dir.path()), UiSettings::default());
        // Unknown fields are ignored, missing ones default, bad values reset.
        std::fs::write(
            path(dir.path()),
            r#"{"unit":"duffs","decimalDigits":12,"autoLockSecs":0,"future":1}"#,
        )
        .unwrap();
        assert_eq!(
            read(dir.path()),
            UiSettings {
                unit: DisplayUnit::Duffs,
                ..Default::default()
            }
        );
    }
}
