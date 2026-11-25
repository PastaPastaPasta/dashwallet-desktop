//! MG's store-upgrade refusal copy, verbatim (mg-store-upgrade-restore-copy.md,
//! sha256 0eb5aec0…7158), as a model the screen renders. `{folder}` is the
//! network directory the refusal names; "upgrade-backups" is the literal
//! folder name (M3-UX (e)).

use crate::error::StoreUpgradeError;
use crate::snapshot::{SnapshotState, UPGRADE_BACKUPS_DIR};

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
            out += &format!("\n{}", hint.replace("{folder}", folder));
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

/// Whether the copy may promise a before-upgrade backup (Q98 98-5): only a
/// [`SnapshotState::Verified`] snapshot; `Unverified` and `None` get the
/// without-backup text.
pub fn backup_sentence(state: &SnapshotState) -> bool {
    matches!(state, SnapshotState::Verified(_))
}

/// `storeUpgrade.cantOpen.*` for `state` (see [`backup_sentence`]).
pub fn cant_open_for(cause: CantOpenCause, state: &SnapshotState) -> ScreenCopy {
    cant_open(cause, backup_sentence(state))
}

/// `storeUpgrade.cantOpen.*`, the same for both causes, with or without the
/// backup sentence.
pub(crate) fn cant_open(_cause: CantOpenCause, backup_present: bool) -> ScreenCopy {
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

const FOREIGN_TITLE: &str = "These wallet files don't match the restore";
const FOREIGN_TEXT: &str = "Dash Wallet was restoring your wallet data from before the upgrade, but the wallet files now in {folder} don't match that data, so it stopped without changing them. Another version of Dash Wallet or another program may have replaced them.";
const FOREIGN_KEEP_MEANING: &str = "This stops the restore and opens the files in {folder} as they are now. Nothing is deleted: the wallet data saved before the upgrade, and the copy taken when the restore started, stay in the upgrade-backups folder.";

/// `storeUpgrade.restore.foreign.*` (MG Q93 §1, mg-q93-keep-and-safe-to-delete.md
/// sha256 b9fe9d5a…49ee): the S4 lineage refusal. "Keep the current files"
/// runs [`crate::keep_current_files`] and is not the default focus; "Report a
/// problem" shows `settings.about.links.issuesHint` beside it.
pub fn restore_foreign() -> ScreenCopy {
    ScreenCopy {
        title_key: "storeUpgrade.restore.foreign.title",
        title: FOREIGN_TITLE,
        text_key: "storeUpgrade.restore.foreign.text",
        text: FOREIGN_TEXT,
        buttons: vec![
            button(
                "storeUpgrade.restore.foreign.keep",
                "Keep the current files",
            ),
            button("settings.about.links.issues", "Report a problem"),
            button("storeUpgrade.cantOpen.quit", "Quit"),
        ],
        hint: Some((
            "storeUpgrade.restore.foreign.keepMeaning",
            FOREIGN_KEEP_MEANING,
        )),
    }
}

/// The screen for an error from this crate, if it has one, worded for the
/// network dir's [`crate::snapshot_state`].
pub fn screen_for(e: &StoreUpgradeError, state: &SnapshotState) -> Option<ScreenCopy> {
    if let StoreUpgradeError::RestoreIncomplete {
        cause: crate::restore::IncompleteCause::S4OtherLineage,
        ..
    } = e
    {
        return Some(restore_foreign());
    }
    CantOpenCause::of(e).map(|c| cant_open_for(c, state))
}

/// The folder name the copy names: the one the snapshot is written under.
pub const COPY_FOLDER_NAME: &str = UPGRADE_BACKUPS_DIR;
