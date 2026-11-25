//! MG's store-upgrade refusal copy, verbatim (mg-store-upgrade-restore-copy.md,
//! sha256 0eb5aec0…7158), as a model the screen renders. `{folder}` is the
//! network directory the refusal names; "upgrade-backups" is the literal
//! folder name (M3-UX (e)).

use crate::error::StoreUpgradeError;
use crate::snapshot::UPGRADE_BACKUPS_DIR;

/// One button: its catalog key and label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    pub key: &'static str,
    pub label: &'static str,
    pub destructive: bool,
    pub default_focus: bool,
}

/// A screen's copy with its catalog keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenCopy {
    pub title_key: &'static str,
    pub title: &'static str,
    pub text_key: &'static str,
    pub text: &'static str,
    pub buttons: Vec<Button>,
    /// A line under the buttons: key and text.
    pub hint: Option<(&'static str, &'static str)>,
}

impl ScreenCopy {
    /// Title, text with `{folder}` filled in, buttons, hint: what the screen
    /// shows.
    pub fn render(&self, folder: &str) -> String {
        let mut out = format!(
            "{}\n\n{}\n",
            self.title,
            self.text.replace("{folder}", folder)
        );
        for b in &self.buttons {
            out += &format!("[{}]", b.label);
        }
        if let Some((_, hint)) = self.hint {
            out += &format!("\n{hint}");
        }
        out
    }
}

const fn button(key: &'static str, label: &'static str) -> Button {
    Button {
        key,
        label,
        destructive: false,
        default_focus: false,
    }
}

const CANT_OPEN_TITLE: &str = "Dash Wallet can't open this wallet data";
const CANT_OPEN_TEXT: &str = "This version of Dash Wallet can't open the wallet data in {folder}. Nothing in it was changed, and the wallet data saved before the upgrade is still in the upgrade-backups folder inside {folder}. Your funds are held on the network, not in this data. Close this version and keep using, or reinstall, the version of Dash Wallet you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.";
const CANT_OPEN_TEXT_WITHOUT_BACKUP: &str = "This version of Dash Wallet can't open the wallet data in {folder}. Nothing in it was changed. Your funds are held on the network, not in this data. Close this version and keep using, or reinstall, the version of Dash Wallet you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.";

/// Which refusal the can't-open screen shows for (Q78-A (j)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CantOpenCause {
    /// A fork file with retired contact-pool rows, P9a Absent.
    RetiredPoolWithoutP9a,
    /// The P9a probe read Broken: every wallet is refused.
    P9aBroken,
}

impl CantOpenCause {
    pub fn of(e: &StoreUpgradeError) -> Option<Self> {
        match e {
            StoreUpgradeError::RetiredPoolWithoutAdopter { .. } => {
                Some(Self::RetiredPoolWithoutP9a)
            }
            StoreUpgradeError::P9aBroken { .. } => Some(Self::P9aBroken),
            _ => None,
        }
    }
}

/// `storeUpgrade.cantOpen.*`, the same for both causes; the backup sentence
/// only when a completed before-upgrade snapshot exists
/// ([`crate::snapshot_present`]).
pub fn cant_open(_cause: CantOpenCause, backup_present: bool) -> ScreenCopy {
    let (text_key, text) = if backup_present {
        ("storeUpgrade.cantOpen.text", CANT_OPEN_TEXT)
    } else {
        (
            "storeUpgrade.cantOpen.textWithoutBackup",
            CANT_OPEN_TEXT_WITHOUT_BACKUP,
        )
    };
    ScreenCopy {
        title_key: "storeUpgrade.cantOpen.title",
        title: CANT_OPEN_TITLE,
        text_key,
        text,
        buttons: vec![button("storeUpgrade.cantOpen.quit", "Quit")],
        hint: None,
    }
}

/// The screen for an error from this crate, if it has one.
pub fn screen_for(e: &StoreUpgradeError, backup_present: bool) -> Option<ScreenCopy> {
    CantOpenCause::of(e).map(|c| cant_open(c, backup_present))
}

/// The folder name the copy names: the one the snapshot is written under.
pub const COPY_FOLDER_NAME: &str = UPGRADE_BACKUPS_DIR;
