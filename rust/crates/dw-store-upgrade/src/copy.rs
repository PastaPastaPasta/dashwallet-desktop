//! MG's store-upgrade and restore copy, verbatim (mg-store-upgrade-restore-copy.md,
//! sha256 9f743aa8…35c0, with its Revision applied), as a model the screen
//! renders. `{folder}` is the network directory the screen names;
//! "upgrade-backups" is the literal folder name (M3-UX (e)).

use crate::error::StoreUpgradeError;
use crate::restore::IncompleteCause;
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
    /// A completed snapshot exists but fails verification: the text is the
    /// no-backup variant, and support needs to know.
    pub backup_unverified: bool,
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

fn screen(
    title: (&'static str, &'static str),
    text: (&'static str, &'static str),
    buttons: Vec<Button>,
) -> ScreenCopy {
    ScreenCopy {
        title_key: title.0,
        title: title.1,
        text_key: text.0,
        text: text.1,
        buttons,
        hint: None,
        backup_unverified: false,
    }
}

/// Marks copy that MG has not written yet; a test lists every use.
pub const PLACEHOLDER: &str = "[MG placeholder] ";

const CANT_OPEN_TITLE: &str = "Dash Wallet can't open this wallet data";
const CANT_OPEN_TEXT: &str = "This version of Dash Wallet can't open the wallet data in {folder}. Nothing in it was changed, and the wallet data saved before the upgrade is still in the upgrade-backups folder inside {folder}. Keep {folder} as it is: it may hold the only record of recent payments, and support may need it. Close this version and keep using, or reinstall, the version of Dash Wallet you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.";
const CANT_OPEN_TEXT_WITHOUT_BACKUP: &str = "This version of Dash Wallet can't open the wallet data in {folder}. Nothing in it was changed. Keep {folder} as it is: it may hold the only record of recent payments, and support may need it. Close this version and keep using, or reinstall, the version of Dash Wallet you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.";

const CONFIRM_TITLE: &str = "Restore wallet data from before the upgrade?";
const CONFIRM_TEXT: &str = "Dash Wallet restarts and puts back the wallet data it saved before the upgrade, so the wallet won't show anything recorded since then. Until the wallet has finished syncing, coins and Shielded notes you've already spent may show as unspent. The wallet data recorded since the upgrade is kept in the upgrade-backups folder inside {folder}. Don't delete it: it may hold the only record of a recent payment.

This version of Dash Wallet pauses sending until syncing finishes. An earlier version doesn't. If you open the wallet with one, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.

If Dash Wallet closes before the restore finishes, start this version again before you open an earlier one.";

const REFUSED_TITLE: &str = "The earlier wallet data couldn't be restored";
const REFUSED_TEXT: &str = "Dash Wallet couldn't use the wallet data it saved before the upgrade, so nothing was changed. The wallet is using the same data as before you chose to restore.";

const INCOMPLETE_TITLE: &str = "Dash Wallet couldn't finish restoring your wallet data";
const INCOMPLETE_TEXT: &str = "Dash Wallet stopped partway through putting back the wallet data it saved before the upgrade, so it hasn't opened the wallet data in {folder}. The data saved before the upgrade and the data it was replacing are both still in the upgrade-backups folder inside {folder}: don't delete anything there. Make sure the disk has free space and that Dash Wallet can write to {folder}, then restart Dash Wallet to finish. Don't open an earlier version of Dash Wallet until this one has finished. If it still can't finish, choose Report a problem.";
const ISSUES_HINT: &str = "Reports are public. Never include your recovery phrase, passphrase or private keys, and never send them to anyone who asks.";

const NO_SPACE_TITLE: &str = "Not enough disk space to restore";
const NO_SPACE_TEXT: &str = "Dash Wallet needs more free disk space to restore your wallet data from before the upgrade, so nothing was changed. Free up space on the disk that holds {folder}, then choose to restore again.";

const NOT_STARTED_TITLE: &str =
    "[MG placeholder] Dash Wallet couldn't start restoring your wallet data";
const NOT_STARTED_TEXT: &str = "[MG placeholder] Dash Wallet couldn't start putting back the wallet data it saved before the upgrade, so nothing was changed. Restart Dash Wallet to try again.";

const KEEP_CURRENT_TITLE: &str =
    "[MG placeholder] Dash Wallet couldn't finish restoring your wallet data";
const KEEP_CURRENT_TEXT: &str = "[MG placeholder] Another version of Dash Wallet changed the wallet data in {folder} after the earlier data was put back. You can keep the wallet data as it is now. Every set of wallet data is kept in the upgrade-backups folder inside {folder}: don't delete anything there.";

const SNAPSHOT_MISSING_TITLE: &str =
    "[MG placeholder] Dash Wallet can't open this wallet data safely";
const SNAPSHOT_MISSING_TEXT: &str = "[MG placeholder] This version of Dash Wallet can't safely open the wallet data in {folder}: it started upgrading it but can't find the copy it saved before the upgrade. Dash Wallet didn't change anything in it. Keep {folder} as it is: it may hold the only record of recent payments, and support may need it. Earlier versions of Dash Wallet can't open it either. Choose Report a problem, and don't delete anything in {folder}.";

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

/// The before-upgrade backup, as the copy may describe it: only a verified
/// snapshot is a backup (Q78-A (h)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupState {
    Verified,
    /// A completed snapshot that fails verification: never called a backup.
    Unverified,
    None,
}

impl From<&SnapshotState> for BackupState {
    fn from(s: &SnapshotState) -> Self {
        match s {
            SnapshotState::Verified(_) => Self::Verified,
            SnapshotState::Unverified { .. } => Self::Unverified,
            SnapshotState::None => Self::None,
        }
    }
}

/// `storeUpgrade.cantOpen.*`, the same for both causes; the backup sentence
/// only for a verified before-upgrade snapshot ([`crate::snapshot_state`]).
pub fn cant_open(_cause: CantOpenCause, backup: BackupState) -> ScreenCopy {
    let text = if backup == BackupState::Verified {
        ("storeUpgrade.cantOpen.text", CANT_OPEN_TEXT)
    } else {
        (
            "storeUpgrade.cantOpen.textWithoutBackup",
            CANT_OPEN_TEXT_WITHOUT_BACKUP,
        )
    };
    ScreenCopy {
        backup_unverified: backup == BackupState::Unverified,
        ..screen(
            ("storeUpgrade.cantOpen.title", CANT_OPEN_TITLE),
            text,
            vec![button("storeUpgrade.cantOpen.quit", "Quit")],
        )
    }
}

/// `storeUpgrade.restore.confirm.*` (MG FINAL with R3 (1) and (ii)): every
/// path to a restore goes through it.
pub fn restore_confirm() -> ScreenCopy {
    screen(
        ("storeUpgrade.restore.confirm.title", CONFIRM_TITLE),
        ("storeUpgrade.restore.confirm.text", CONFIRM_TEXT),
        vec![
            Button {
                destructive: true,
                ..button(
                    "storeUpgrade.restore.confirm.confirm",
                    "Restart and restore",
                )
            },
            Button {
                default_focus: true,
                ..button("storeUpgrade.restore.confirm.cancel", "Cancel")
            },
        ],
    )
}

/// `storeUpgrade.restore.refused.*`, the same for every cause (MG (n)).
pub fn restore_refused(_cause: crate::restore::RefusalCause) -> ScreenCopy {
    screen(
        ("storeUpgrade.restore.refused.title", REFUSED_TITLE),
        ("storeUpgrade.restore.refused.text", REFUSED_TEXT),
        vec![button("storeUpgrade.restore.refused.ok", "OK")],
    )
}

/// `storeUpgrade.restore.incomplete.*`, the same for every cause; the cause
/// stays in the error for the report (MG (q)).
pub fn restore_incomplete(_cause: IncompleteCause) -> ScreenCopy {
    ScreenCopy {
        hint: Some(("settings.links.issuesHint", ISSUES_HINT)),
        ..screen(
            ("storeUpgrade.restore.incomplete.title", INCOMPLETE_TITLE),
            ("storeUpgrade.restore.incomplete.text", INCOMPLETE_TEXT),
            vec![
                button("storeUpgrade.restore.incomplete.restart", "Restart"),
                button("storeUpgrade.restore.incomplete.report", "Report a problem"),
                button("storeUpgrade.restore.incomplete.quit", "Quit"),
            ],
        )
    }
}

/// `storeUpgrade.restore.noSpace.*`. The size the copies need travels in
/// [`StoreUpgradeError::RestoreNoSpace`]; MG adds `{size}` on request (MG (u)).
pub fn restore_no_space() -> ScreenCopy {
    screen(
        ("storeUpgrade.restore.noSpace.title", NO_SPACE_TITLE),
        ("storeUpgrade.restore.noSpace.text", NO_SPACE_TEXT),
        vec![button("storeUpgrade.restore.noSpace.ok", "OK")],
    )
}

/// `storeUpgrade.restore.notStarted.*`: a fault before the journal other
/// than disk space; nothing changed and the request is kept (design §7).
pub fn restore_not_started(_cause: IncompleteCause) -> ScreenCopy {
    screen(
        ("storeUpgrade.restore.notStarted.title", NOT_STARTED_TITLE),
        ("storeUpgrade.restore.notStarted.text", NOT_STARTED_TEXT),
        vec![
            button("storeUpgrade.restore.notStarted.restart", "Restart"),
            button("storeUpgrade.restore.notStarted.quit", "Quit"),
        ],
    )
}

/// `storeUpgrade.restore.keepCurrent.*`: S4 refused (Q92 S4-2); the keep
/// button runs [`crate::keep_current_files`].
pub fn restore_keep_current() -> ScreenCopy {
    ScreenCopy {
        hint: Some(("settings.links.issuesHint", ISSUES_HINT)),
        ..screen(
            ("storeUpgrade.restore.keepCurrent.title", KEEP_CURRENT_TITLE),
            ("storeUpgrade.restore.keepCurrent.text", KEEP_CURRENT_TEXT),
            vec![
                button(
                    "storeUpgrade.restore.keepCurrent.keep",
                    "[MG placeholder] Keep the current wallet data",
                ),
                button(
                    "storeUpgrade.restore.keepCurrent.report",
                    "Report a problem",
                ),
                button("storeUpgrade.restore.keepCurrent.quit", "Quit"),
            ],
        )
    }
}

/// `storeUpgrade.snapshotMissing.*`: an upgrade part-done with no snapshot
/// to keep (M1), in the shape of MG's draft refusal family with its
/// `wayOut.support`. The same for every state; the state stays in the error.
pub fn snapshot_missing(_state: crate::snapshot::MissingSnapshot) -> ScreenCopy {
    ScreenCopy {
        hint: Some(("settings.links.issuesHint", ISSUES_HINT)),
        ..screen(
            ("storeUpgrade.snapshotMissing.title", SNAPSHOT_MISSING_TITLE),
            ("storeUpgrade.snapshotMissing.text", SNAPSHOT_MISSING_TEXT),
            vec![
                button("storeUpgrade.snapshotMissing.report", "Report a problem"),
                button("storeUpgrade.snapshotMissing.quit", "Quit"),
            ],
        )
    }
}

/// The screen for an error from this crate, if it has one.
pub fn screen_for(e: &StoreUpgradeError, backup: BackupState) -> Option<ScreenCopy> {
    if let Some(c) = CantOpenCause::of(e) {
        return Some(cant_open(c, backup));
    }
    Some(match e {
        StoreUpgradeError::UpgradeSnapshotMissing { state, .. } => snapshot_missing(*state),
        StoreUpgradeError::RestoreRefused { cause } => restore_refused(*cause),
        StoreUpgradeError::RestoreNoSpace { .. } => restore_no_space(),
        StoreUpgradeError::RestoreIncomplete {
            cause: IncompleteCause::S4OldBuildWrote | IncompleteCause::S4OtherLineage,
            journal_written: true,
            ..
        } => restore_keep_current(),
        StoreUpgradeError::RestoreIncomplete {
            cause,
            journal_written: true,
            ..
        } => restore_incomplete(*cause),
        StoreUpgradeError::RestoreIncomplete { cause, .. } => restore_not_started(*cause),
        _ => return None,
    })
}

/// Every screen this module renders, for catalog checks.
pub fn all_screens() -> Vec<ScreenCopy> {
    use crate::restore::RefusalCause;
    let mut v = vec![
        cant_open(CantOpenCause::P9aBroken, BackupState::Verified),
        cant_open(CantOpenCause::P9aBroken, BackupState::None),
        restore_confirm(),
        restore_refused(RefusalCause::MissingBackup),
        restore_incomplete(IncompleteCause::Other),
        restore_no_space(),
        restore_not_started(IncompleteCause::Other),
        restore_keep_current(),
        snapshot_missing(crate::snapshot::MissingSnapshot::Absent),
    ];
    v.dedup();
    v
}

/// The folder name the copy names: the one the snapshot is written under.
pub const COPY_FOLDER_NAME: &str = UPGRADE_BACKUPS_DIR;
