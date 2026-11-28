//! The renderer's display settings (U-04 D2): `<data root>/settings.json`,
//! next to `window-state.json`. One file for the data root, not per network.
//! A missing or unreadable file reads as the defaults; a write replaces the
//! file atomically (a temporary file renamed over it). Nothing secret is
//! kept here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
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
    /// Settings ▸ General ▸ Show Masternodes (MG-00 D1, D2 as amended by
    /// MG-10 Q5): the Masternodes sidebar item and route. Auto shows it when
    /// the wallet holds masternode keys, a collateral-shaped coin, a tracked
    /// masternode or a proposal in progress. A file from before this setting
    /// reads `showMasternodes: true` as On and anything else as Auto.
    #[serde(deserialize_with = "MasternodesItem::lenient")]
    pub masternodes_item: MasternodesItem,
    /// The Masternodes page's open tab (MG-10 D1); a tab this build doesn't
    /// know opens the default.
    #[serde(deserialize_with = "MasternodesTab::lenient")]
    pub masternodes_tab: MasternodesTab,
    /// The 0.1 pre-release notice was dismissed: it is shown once, on the
    /// first run.
    pub pre_release_notice_dismissed: bool,
    /// The user interface language as a BCP 47 tag (`"en"`, `"pt-BR"`);
    /// `null` follows the system (MP-14). The renderer decides which tags it
    /// has a catalog for.
    pub language: Option<String>,
    /// Settings ▸ About ▸ Check for updates automatically (PK-03): off until
    /// the user turns it on; only a copy that can update itself checks.
    pub check_updates_automatically: bool,
    /// Networks on which the user enabled experimental gift cards.
    pub gift_cards_networks: Vec<String>,
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
            masternodes_item: MasternodesItem::Auto,
            masternodes_tab: MasternodesTab::MyMasternodes,
            pre_release_notice_dismissed: false,
            language: None,
            check_updates_automatically: false,
            gift_cards_networks: Vec::new(),
        }
    }
}

/// The longest language tag a file may hold.
const MAX_LANGUAGE_LEN: usize = 35;

/// The language tags this app accepts: a deliberately narrower grammar than
/// BCP 47. A 2–3 letter language, then subtags of 1–8 letters or digits; a
/// single-character subtag (an extension or private-use singleton) must be
/// followed by a longer subtag. Private-use-only tags (`x-wallet`) and the
/// irregular grandfathered tags (`i-klingon`) are refused; this is a check
/// of shape, not of the registry.
fn is_language_tag(tag: &str) -> bool {
    if tag.len() > MAX_LANGUAGE_LEN {
        return false;
    }
    let mut parts = tag.split('-');
    let lang = parts.next().unwrap_or_default();
    if !(2..=3).contains(&lang.len()) || !lang.bytes().all(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    let mut open_singleton = false;
    for p in parts {
        if !(1..=8).contains(&p.len()) || !p.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return false;
        }
        if p.len() == 1 && open_singleton {
            return false;
        }
        open_singleton = p.len() == 1;
    }
    !open_singleton
}

impl MasternodesItem {
    fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        Ok(Self::deserialize(value).unwrap_or_default())
    }
}

/// When the Masternodes sidebar item shows (MG-00 D2, amended by MG-10 Q5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MasternodesItem {
    #[default]
    Auto,
    On,
    Off,
}

impl MasternodesTab {
    fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        Ok(Self::deserialize(value).unwrap_or_default())
    }
}

/// A tab of the Masternodes page (MG-00 D3 order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MasternodesTab {
    #[default]
    MyMasternodes,
    Governance,
    Names,
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
        if self
            .language
            .as_deref()
            .is_some_and(|l| !is_language_tag(l))
        {
            return Err(AppError::invalid_argument("language must be a BCP 47 tag"));
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
        if self
            .language
            .as_deref()
            .is_some_and(|l| !is_language_tag(l))
        {
            self.language = defaults.language;
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
    match from_file(&text) {
        Ok(s) => s.sanitized(),
        Err(e) => {
            eprintln!("dw-app: {FILE} is unreadable, using the defaults: {e}");
            UiSettings::default()
        }
    }
}

/// The file's settings. A `language` of the wrong type reads as `null`
/// (follow the system) rather than failing the whole file: the other
/// settings, auto-lock among them, stay as saved. The fields are split as
/// raw text, so a `language` of any shape or depth is set aside unparsed.
/// IPC writes deserialize `UiSettings` directly and stay strict.
fn from_file(text: &str) -> serde_json::Result<UiSettings> {
    let Ok(mut fields) = serde_json::from_str::<BTreeMap<String, &RawValue>>(text) else {
        return serde_json::from_str(text);
    };
    let language = fields
        .remove("language")
        .and_then(|raw| serde_json::from_str::<Option<String>>(raw.get()).ok())
        .flatten();
    let legacy_on = !fields.contains_key("masternodesItem")
        && fields
            .remove("showMasternodes")
            .is_some_and(|raw| raw.get() == "true");
    let mut settings: UiSettings = serde_json::from_str(&serde_json::to_string(&fields)?)?;
    settings.language = language;
    if legacy_on {
        settings.masternodes_item = MasternodesItem::On;
    }
    Ok(settings)
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
            masternodes_item: MasternodesItem::Off,
            masternodes_tab: MasternodesTab::Names,
            pre_release_notice_dismissed: true,
            language: Some("pt-BR".into()),
            check_updates_automatically: true,
            gift_cards_networks: vec!["mainnet".into()],
        };
        write(dir.path(), &s).unwrap();
        assert_eq!(read(dir.path()), s);
        assert!(!dir.path().join("settings.json.tmp").exists());
    }

    #[test]
    fn a_file_without_the_tab_opens_my_masternodes() {
        let dir = root();
        std::fs::write(path(dir.path()), r#"{"showMasternodes":true}"#).unwrap();
        assert_eq!(
            read(dir.path()).masternodes_tab,
            MasternodesTab::MyMasternodes
        );
    }

    #[test]
    fn an_unknown_tab_opens_my_masternodes_and_keeps_the_rest() {
        let dir = root();
        for tab in [r#""keys""#, "7", "null"] {
            let file = format!(r#"{{"showMasternodes":true,"masternodesTab":{tab}}}"#);
            std::fs::write(path(dir.path()), file).unwrap();
            let s = read(dir.path());
            assert_eq!(s.masternodes_tab, MasternodesTab::MyMasternodes);
            assert_eq!(s.masternodes_item, MasternodesItem::On);
        }
    }

    #[test]
    fn show_masternodes_from_an_older_file_reads_as_on_and_anything_else_as_auto() {
        let dir = root();
        let cases = [
            (r#"{"showMasternodes":true}"#, MasternodesItem::On),
            (r#"{"showMasternodes":false}"#, MasternodesItem::Auto),
            (r#"{"showMasternodes":"yes"}"#, MasternodesItem::Auto),
            (r#"{"unit":"duffs"}"#, MasternodesItem::Auto),
            (
                r#"{"showMasternodes":true,"masternodesItem":"off"}"#,
                MasternodesItem::Off,
            ),
            (r#"{"masternodesItem":"on"}"#, MasternodesItem::On),
            (r#"{"masternodesItem":"sometimes"}"#, MasternodesItem::Auto),
        ];
        for (file, want) in cases {
            std::fs::write(path(dir.path()), file).unwrap();
            assert_eq!(read(dir.path()).masternodes_item, want, "{file}");
        }
        assert_eq!(
            UiSettings::default().masternodes_item,
            MasternodesItem::Auto
        );
    }

    #[test]
    fn a_saved_file_no_longer_holds_show_masternodes() {
        let dir = root();
        write(dir.path(), &UiSettings::default()).unwrap();
        let text = std::fs::read_to_string(path(dir.path())).unwrap();
        assert!(!text.contains("showMasternodes"), "{text}");
        assert!(text.contains(r#""masternodesItem": "auto""#), "{text}");
    }

    #[test]
    fn the_pre_release_notice_shows_until_dismissed() {
        let dir = root();
        assert!(!UiSettings::default().pre_release_notice_dismissed);
        // A file from before the notice has no such field: not dismissed.
        std::fs::write(path(dir.path()), r#"{"unit":"duffs"}"#).unwrap();
        assert!(!read(dir.path()).pre_release_notice_dismissed);
        let s = UiSettings {
            pre_release_notice_dismissed: true,
            ..Default::default()
        };
        write(dir.path(), &s).unwrap();
        assert!(read(dir.path()).pre_release_notice_dismissed);
    }

    #[test]
    fn automatic_update_checks_are_off_until_turned_on() {
        let dir = root();
        assert!(!UiSettings::default().check_updates_automatically);
        std::fs::write(path(dir.path()), r#"{"unit":"duffs"}"#).unwrap();
        assert!(!read(dir.path()).check_updates_automatically);
    }

    #[test]
    fn a_language_tag_is_kept_and_a_missing_one_follows_the_system() {
        let dir = root();
        std::fs::write(path(dir.path()), r#"{"unit":"duffs"}"#).unwrap();
        assert_eq!(read(dir.path()).language, None);
        for tag in [
            "en",
            "pt-BR",
            "zh-Hans",
            "ar-XB",
            "es-419",
            "de-u-co-phonebk",
            "en-x-wallet",
        ] {
            let s = UiSettings {
                language: Some(tag.into()),
                ..Default::default()
            };
            write(dir.path(), &s).unwrap();
            assert_eq!(read(dir.path()).language.as_deref(), Some(tag));
        }
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
            UiSettings {
                language: Some(String::new()),
                ..Default::default()
            },
            UiSettings {
                language: Some("en_US".into()),
                ..Default::default()
            },
            UiSettings {
                language: Some("e".into()),
                ..Default::default()
            },
            UiSettings {
                language: Some(format!("en-{}", "a".repeat(40))),
                ..Default::default()
            },
            UiSettings {
                language: Some("en-u".into()),
                ..Default::default()
            },
            UiSettings {
                language: Some("en-x".into()),
                ..Default::default()
            },
            UiSettings {
                language: Some("en-u-x-ab".into()),
                ..Default::default()
            },
            UiSettings {
                language: Some("x-wallet".into()),
                ..Default::default()
            },
        ] {
            let e = write(dir.path(), &bad).unwrap_err();
            assert_eq!(e.code_str(), "invalid_argument");
        }
        assert!(!path(dir.path()).exists());
    }

    #[test]
    fn a_wrong_typed_language_keeps_the_other_settings() {
        let dir = root();
        let nested = format!("{}{}", "[".repeat(300), "]".repeat(300));
        for language in ["7", "true", "[]", r#"{"tag":"en"}"#, nested.as_str()] {
            let file = format!(
                r#"{{"autoLockSecs":300,"autohideBalance":true,"unit":"duffs","language":{language}}}"#
            );
            std::fs::write(path(dir.path()), file).unwrap();
            let s = read(dir.path());
            assert_eq!(s.auto_lock_secs, Some(300));
            assert!(s.autohide_balance);
            assert_eq!(s.unit, DisplayUnit::Duffs);
            assert_eq!(s.language, None);
        }
    }

    #[test]
    fn an_ipc_write_with_a_wrong_typed_language_is_refused() {
        let ipc = r#"{"autoLockSecs":300,"autohideBalance":true,"unit":"duffs","language":7}"#;
        assert!(serde_json::from_str::<UiSettings>(ipc).is_err());
    }

    #[test]
    fn an_overlong_language_is_refused_before_it_is_scanned() {
        assert!(!is_language_tag(&"e".repeat(MAX_LANGUAGE_LEN + 1)));
        assert!(!is_language_tag(&format!("en-{}", "-".repeat(64))));
        assert!(is_language_tag("en-Latn-US"));
    }

    #[test]
    fn an_unreadable_or_hand_edited_file_falls_back() {
        let dir = root();
        std::fs::write(path(dir.path()), "{not json").unwrap();
        assert_eq!(read(dir.path()), UiSettings::default());
        // Unknown fields are ignored, missing ones default, bad values reset.
        std::fs::write(
            path(dir.path()),
            r#"{"unit":"duffs","decimalDigits":12,"autoLockSecs":0,"language":"../x","future":1}"#,
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
