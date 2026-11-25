//! MG's store-upgrade copy, verbatim, as a model the screen renders.
//! `{folder}` is the network directory the screen names; "upgrade-backups"
//! is the literal folder name (M3-UX (e)).
//!
//! The restore flow's screens (mg-store-upgrade-restore-copy.md, sha256
//! 9f743aa8…35c0, with its Revision applied) are [`ScreenCopy`]s, chosen by
//! [`restore_screen_for`]. Keys MG has not written yet are listed in
//! [`PENDING_MG`] or start with [`PLACEHOLDER`].
//!
//! The refusal at open (`storeUpgrade.refused.*`, MG's final cantOpen
//! family, sha256 8c2c34db…0da2) is title + "{intro}: {detail} {unchanged}
//! {wayOut}". The detail follows the refusal's cause; the way out follows
//! the file's proved state, never the cause alone, and an unproved state
//! never offers the earlier version. "{unchanged}" says Dash Wallet changed
//! nothing only for a file proved unchanged by this start; otherwise it is
//! `storeUpgrade.refused.changedByUpgrade`.

use std::path::Path;

use crate::error::StoreUpgradeError;
use crate::restore::IncompleteCause;
use crate::snapshot::{MissingSnapshot, STORE_FILES, SnapshotState, UPGRADE_BACKUPS_DIR};

/// One button: its catalog key and label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    pub key: &'static str,
    pub label: &'static str,
    pub destructive: bool,
    pub default_focus: bool,
}

const fn button(key: &'static str, label: &'static str) -> Button {
    Button {
        key,
        label,
        destructive: false,
        default_focus: false,
    }
}

/// One catalog string: its key and English text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub key: &'static str,
    pub text: &'static str,
}

const fn line(key: &'static str, text: &'static str) -> Line {
    Line { key, text }
}

/// `settings.about.links.issuesHint`, shown under every "Report a problem".
pub const ISSUES_HINT: Line = line(
    "settings.about.links.issuesHint",
    "Reports are public. Never include your recovery phrase, passphrase or private keys, and never send them to anyone who asks.",
);

const REFUSED_TITLE: Line = line(
    "storeUpgrade.refused.title",
    "Dash Wallet can't open this wallet data safely",
);
const REFUSED_INTRO: Line = line(
    "storeUpgrade.refused.intro",
    "This version of Dash Wallet can't safely open the wallet data in {folder}",
);
const REFUSED_UNCHANGED: Line = line(
    "storeUpgrade.refused.unchanged",
    "Dash Wallet didn't change anything in it. Keep {folder} as it is: it may hold the only record of recent payments, and support may need it.",
);
const CHANGED_BY_UPGRADE: Line = line(
    "storeUpgrade.refused.changedByUpgrade",
    "Dash Wallet had started upgrading it when it stopped. Keep {folder} as it is: it may hold the only record of recent payments, and support may need it.",
);

const QUIT: Button = button("storeUpgrade.refused.quit", "Quit");
const REPORT: Button = button("storeUpgrade.refused.report", "Report a problem");
const RESTORE: Button = button("storeUpgrade.refused.restore", "Restore earlier data");

/// The screen "Restore earlier data" opens: the R3 confirm, the only way to
/// a restore (MG (D)).
pub const RESTORE_BUTTON_OPENS: &str = "storeUpgrade.restore.confirm";

/// Why the store was refused: picks `storeUpgrade.refused.detail.*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefusedCause {
    /// [`StoreUpgradeError::ForkStoreUnsupported`].
    ForkUnsupported,
    /// [`StoreUpgradeError::ForkStoreShapeMismatch`].
    ShapeMismatch,
    /// [`StoreUpgradeError::UpgradeRehearsalFailed`].
    RehearsalFailed,
    /// [`StoreUpgradeError::PreHistoryFork`].
    DevBuild,
    /// [`StoreUpgradeError::SchemaTooNew`].
    TooNew,
    /// [`StoreUpgradeError::SchemaHistoryMismatch`].
    HistoryMismatch,
    /// [`StoreUpgradeError::UpgradeSnapshotMissing`]: part-upgraded, so
    /// migrated whatever the state says, and with no snapshot to restore.
    /// Detail pending MG.
    SnapshotMissing,
    /// [`StoreUpgradeError::RetiredPoolWithoutAdopter`].
    ContactsTable,
    /// [`StoreUpgradeError::P9aBroken`]: this build's own check failed. It
    /// refuses every file, migrated or not, and may be a passing fault such
    /// as a full disk (Q98 98-3): only a file proved not yet upgraded may go
    /// back to the earlier version.
    SelfCheck,
    /// Every other error that leaves the files unchanged and is not retried.
    Unexpected,
}

impl RefusedCause {
    pub const ALL: [Self; 10] = [
        Self::ForkUnsupported,
        Self::ShapeMismatch,
        Self::RehearsalFailed,
        Self::DevBuild,
        Self::TooNew,
        Self::HistoryMismatch,
        Self::SnapshotMissing,
        Self::ContactsTable,
        Self::SelfCheck,
        Self::Unexpected,
    ];

    /// The cause the refusal screen shows for `e`, or `None` when `e` gets
    /// no refusal screen: a retryable fault (the store held by another
    /// process, the snapshot not written, an SQLite or I/O error), a pending
    /// restore or any restore error (the restore screens own them,
    /// [`restore_screen_for`]), or an error after which the files may have
    /// changed.
    pub fn of(e: &StoreUpgradeError) -> Option<Self> {
        use StoreUpgradeError as E;
        if !e.file_unchanged() {
            return None;
        }
        Some(match e {
            E::ForkStoreUnsupported { .. } => Self::ForkUnsupported,
            E::ForkStoreShapeMismatch { .. } => Self::ShapeMismatch,
            E::UpgradeRehearsalFailed { .. } => Self::RehearsalFailed,
            E::PreHistoryFork => Self::DevBuild,
            E::SchemaTooNew { .. } => Self::TooNew,
            E::SchemaHistoryMismatch { .. } => Self::HistoryMismatch,
            E::UpgradeSnapshotMissing { .. } => Self::SnapshotMissing,
            E::RetiredPoolWithoutAdopter { .. } => Self::ContactsTable,
            E::P9aBroken { .. } => Self::SelfCheck,
            E::StoreHeld { .. }
            | E::SnapshotFailed { .. }
            | E::Sqlite { .. }
            | E::Io { .. }
            | E::RestorePending { .. }
            | E::RestoreRefused { .. }
            | E::RestoreNoSpace { .. }
            | E::RestoreIncomplete { .. }
            | E::RestoreAfterOpen => return None,
            _ => Self::Unexpected,
        })
    }

    pub fn detail(self) -> Line {
        match self {
            Self::ForkUnsupported => line(
                "storeUpgrade.refused.detail.forkUnsupported",
                "it was written by a version of Dash Wallet that this one can't upgrade from.",
            ),
            Self::ShapeMismatch => line(
                "storeUpgrade.refused.detail.shapeMismatch",
                "its layout doesn't match the version it says it is, so another program or a test build may have changed it.",
            ),
            Self::RehearsalFailed => line(
                "storeUpgrade.refused.detail.rehearsalFailed",
                "a trial upgrade on a copy of it failed, so Dash Wallet left the real data as it was.",
            ),
            Self::DevBuild => line(
                "storeUpgrade.refused.detail.devBuild",
                "it was written by a development build of Dash Wallet.",
            ),
            Self::TooNew => line(
                "storeUpgrade.refused.detail.tooNew",
                "it was written by a newer version of Dash Wallet.",
            ),
            Self::HistoryMismatch => line(
                "storeUpgrade.refused.detail.historyMismatch",
                "its upgrade history doesn't match any released version of Dash Wallet, so this version can't tell what it contains.",
            ),
            Self::SnapshotMissing => line(
                "storeUpgrade.refused.detail.snapshotMissing",
                "an upgrade of it was interrupted and the copy taken before it is missing or doesn't check out.",
            ),
            Self::ContactsTable => line(
                "storeUpgrade.refused.detail.contactsTable",
                "it holds contact data that this version can't read yet.",
            ),
            Self::SelfCheck => line(
                "storeUpgrade.refused.detail.selfCheck",
                "a check Dash Wallet runs on itself before it opens any wallet data didn't pass.",
            ),
            Self::Unexpected => line(
                "storeUpgrade.refused.detail.unexpected",
                "it's in a state this version doesn't expect.",
            ),
        }
    }
}

/// The before-upgrade backup, as the engine proved it: only a verified
/// snapshot is a backup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupState {
    /// A completed snapshot whose files all match their sizes and sha256.
    Verified,
    /// A completed snapshot that fails verification, and no verified one.
    Unverified,
    /// No completed snapshot.
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

/// What the engine proved about the refused file. `None` in a field means
/// not proved, which never offers the earlier version (MG (A)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProvedState {
    /// A new build migrated it (refinery's `MAX(version)` past V018) or
    /// part-upgraded it, so no earlier version opens it ([`migrated`]).
    pub migrated: Option<bool>,
    pub backup: Option<BackupState>,
    /// This start wrote nothing to it and it is not part-upgraded: only then
    /// does the screen say Dash Wallet didn't change anything.
    pub unchanged: bool,
}

impl ProvedState {
    /// The state of `network_dir` for a refusal this start, read without
    /// writing. `wrote` is whether this start's reconcile wrote to it.
    pub fn read(network_dir: &Path, wrote: bool) -> Self {
        let upgrade = upgrade(network_dir);
        Self {
            migrated: upgrade.map(|u| u != Upgrade::NotStarted),
            backup: crate::snapshot::snapshot_state(network_dir)
                .ok()
                .as_ref()
                .map(BackupState::from),
            unchanged: !wrote && matches!(upgrade, Some(Upgrade::NotStarted | Upgrade::Done)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Upgrade {
    NotStarted,
    Part,
    Done,
}

/// Whether `network_dir`'s `wallet.sqlite` was changed past what a fork
/// build opens, read with `immutable=1`: migrated past the last version a
/// fork build wrote (refinery `MAX(version)` > 18), or part-upgraded (fork
/// history present and R1's drop of `contacts.external_account_reference`
/// committed; every published fork version set adds that column, and no
/// released build opens the file without it). `None` when that can't be
/// proved: no file, no refinery history or `contacts`, a read error, or an
/// unchanged-looking file with a non-empty `-wal` or `-journal` the
/// immutable read can't see.
pub fn migrated(network_dir: &Path) -> Option<bool> {
    upgrade(network_dir).map(|u| u != Upgrade::NotStarted)
}

fn upgrade(network_dir: &Path) -> Option<Upgrade> {
    let path = network_dir.join(STORE_FILES[0]);
    if !path.is_file() {
        return None;
    }
    let conn = crate::sql::open_immutable(&path).ok()?;
    if !crate::sql::has_table(&conn, "refinery_schema_history").ok()? {
        return None;
    }
    let max: i64 = conn
        .query_row(
            "SELECT MAX(version) FROM refinery_schema_history",
            [],
            |r| r.get::<_, Option<i64>>(0),
        )
        .ok()??;
    if max > crate::fork::FORK_REFINERY_MAX {
        return Some(Upgrade::Done);
    }
    if crate::sql::has_table(&conn, crate::fork::FORK_HISTORY).ok()? {
        if !crate::sql::has_table(&conn, crate::fork::CONTACTS).ok()? {
            return None;
        }
        if !crate::sql::has_column(&conn, crate::fork::CONTACTS, crate::fork::EXTERNAL_REF).ok()? {
            return Some(Upgrade::Part);
        }
    }
    let logged = ["-wal", "-journal"].iter().any(|suffix| {
        std::fs::metadata(network_dir.join(format!("{}{suffix}", STORE_FILES[0])))
            .is_ok_and(|m| m.len() > 0)
    });
    (!logged).then_some(Upgrade::NotStarted)
}

/// The way out the screen offers: `storeUpgrade.refused.wayOut.*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WayOut {
    /// Not migrated: the earlier version opens it.
    Previous,
    /// [`RefusedCause::TooNew`] only.
    Newer,
    /// Migrated, backup verified.
    Restore,
    /// Migrated, backup unverified.
    Unverified,
    /// Not proved migrated, and nothing better proved.
    Support,
    /// Migrated, no backup or the backup not proved.
    SupportMigrated,
    /// [`RefusedCause::SelfCheck`], the file migrated or not proved either
    /// way: never the earlier version.
    SelfCheck,
    /// [`RefusedCause::SelfCheck`], the file proved not yet upgraded.
    SelfCheckNotMigrated,
}

impl WayOut {
    pub const ALL: [Self; 8] = [
        Self::Previous,
        Self::Newer,
        Self::Restore,
        Self::Unverified,
        Self::Support,
        Self::SupportMigrated,
        Self::SelfCheck,
        Self::SelfCheckNotMigrated,
    ];

    /// MG's cause → allowed ways out, by the proved state.
    pub fn for_cause(cause: RefusedCause, state: ProvedState) -> Self {
        use RefusedCause as C;
        let by_state = || match (state.migrated, state.backup) {
            (Some(true), Some(BackupState::Verified)) => Self::Restore,
            (Some(true), Some(BackupState::Unverified)) => Self::Unverified,
            (Some(true), _) => Self::SupportMigrated,
            _ => Self::Support,
        };
        match cause {
            C::TooNew => Self::Newer,
            C::SelfCheck if state.migrated == Some(false) => Self::SelfCheckNotMigrated,
            C::SelfCheck => Self::SelfCheck,
            C::ForkUnsupported | C::RehearsalFailed => match state.migrated {
                Some(false) => Self::Previous,
                Some(true) => Self::SupportMigrated,
                None => Self::Support,
            },
            C::SnapshotMissing => match state.backup {
                Some(BackupState::Unverified) => Self::Unverified,
                _ => Self::SupportMigrated,
            },
            C::ContactsTable if state.migrated == Some(false) => Self::Previous,
            C::ContactsTable
            | C::ShapeMismatch
            | C::DevBuild
            | C::HistoryMismatch
            | C::Unexpected => by_state(),
        }
    }

    pub fn line(self) -> Line {
        match self {
            Self::Previous => line(
                "storeUpgrade.refused.wayOut.previous",
                "Close this version and keep using, or reinstall, the version of Dash Wallet you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.",
            ),
            Self::Newer => line(
                "storeUpgrade.refused.wayOut.newer",
                "Install that version of Dash Wallet, or a later one, to open it. Don't open it with an earlier version.",
            ),
            Self::Restore => line(
                "storeUpgrade.refused.wayOut.restore",
                "Earlier versions of Dash Wallet can't open it either, because it has already been upgraded. To go back to your wallet data from before the upgrade, choose Restore earlier data. Otherwise, choose Report a problem.",
            ),
            Self::Unverified => line(
                "storeUpgrade.refused.wayOut.unverified",
                "Earlier versions of Dash Wallet can't open it either, because it has already been upgraded. A backup from before the upgrade exists, but it doesn't check out, so Dash Wallet won't restore from it. Choose Report a problem.",
            ),
            Self::Support => line(
                "storeUpgrade.refused.wayOut.support",
                "Choose Report a problem.",
            ),
            Self::SupportMigrated => line(
                "storeUpgrade.refused.wayOut.supportMigrated",
                "Earlier versions of Dash Wallet can't open it either, because it has already been upgraded. Choose Report a problem.",
            ),
            Self::SelfCheck => line(
                "storeUpgrade.refused.wayOut.selfCheck",
                "Quit Dash Wallet and open it again. If you see this again, free up some disk space and try once more. If it still happens, install the latest version of Dash Wallet. Until then, don't open this wallet data with an earlier version.",
            ),
            Self::SelfCheckNotMigrated => line(
                "storeUpgrade.refused.wayOut.selfCheckNotMigrated",
                "Quit Dash Wallet and open it again. If you see this again, free up some disk space and try once more. If it still happens, install the latest version of Dash Wallet, or keep using the version you used before. If you do, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.",
            ),
        }
    }

    /// Restore earlier data (only with [`Self::Restore`]), Report a problem
    /// (except with [`Self::Previous`] and [`Self::Newer`]), Quit always.
    fn buttons(self) -> Vec<Button> {
        let mut out = Vec::new();
        if self == Self::Restore {
            out.push(RESTORE);
        }
        if self.reports() {
            out.push(REPORT);
        }
        out.push(QUIT);
        out
    }

    fn reports(self) -> bool {
        !matches!(self, Self::Previous | Self::Newer)
    }
}

/// The refusal screen: what it shows and the keys it shows it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefusedScreen {
    pub cause: RefusedCause,
    pub way_out: WayOut,
    pub title: Line,
    pub intro: Line,
    pub detail: Line,
    /// `storeUpgrade.refused.unchanged` for a file proved unchanged by this
    /// start ([`ProvedState::unchanged`]), else `changedByUpgrade`.
    pub unchanged: Line,
    pub way_out_line: Line,
    pub buttons: Vec<Button>,
    /// [`ISSUES_HINT`], under the buttons with Report a problem.
    pub hint: Option<Line>,
}

impl RefusedScreen {
    pub fn new(cause: RefusedCause, state: ProvedState) -> Self {
        let way_out = WayOut::for_cause(cause, state);
        Self {
            cause,
            way_out,
            title: REFUSED_TITLE,
            intro: REFUSED_INTRO,
            detail: cause.detail(),
            unchanged: if state.unchanged {
                REFUSED_UNCHANGED
            } else {
                CHANGED_BY_UPGRADE
            },
            way_out_line: way_out.line(),
            buttons: way_out.buttons(),
            hint: way_out.reports().then_some(ISSUES_HINT),
        }
    }

    /// "{intro}: {detail} {unchanged} {wayOut}" with `{folder}` filled in.
    pub fn text(&self, folder: &str) -> String {
        format!(
            "{}: {} {} {}",
            self.intro.text, self.detail.text, self.unchanged.text, self.way_out_line.text
        )
        .replace("{folder}", folder)
    }

    /// Title, text, buttons, hint: what the screen shows.
    pub fn render(&self, folder: &str) -> String {
        let mut out = format!("{}\n\n{}\n", self.title.text, self.text(folder));
        for b in &self.buttons {
            out += &format!("[{}]", b.label);
        }
        if let Some(hint) = self.hint {
            out += &format!("\n{}", hint.text);
        }
        out
    }
}

/// The refusal screen for an error from this crate, if it has one
/// ([`RefusedCause::of`]), its way out chosen by `state`. The caller proves
/// `state`, normally with [`ProvedState::read`].
pub fn screen_for(e: &StoreUpgradeError, state: ProvedState) -> Option<RefusedScreen> {
    let cause = RefusedCause::of(e)?;
    let state = match e {
        StoreUpgradeError::UpgradeSnapshotMissing { state: missing, .. } => {
            snapshot_missing_state(*missing)
        }
        _ => state,
    };
    Some(RefusedScreen::new(cause, state))
}

/// A part-done upgrade is migrated and changed, and its backup is what the
/// error found: a snapshot of other data is no backup of this wallet.
fn snapshot_missing_state(missing: MissingSnapshot) -> ProvedState {
    ProvedState {
        migrated: Some(true),
        unchanged: false,
        backup: Some(match missing {
            MissingSnapshot::Unverified => BackupState::Unverified,
            MissingSnapshot::Absent | MissingSnapshot::Mismatch => BackupState::None,
        }),
    }
}

/// Catalog keys whose text is a best guess until MG writes the final.
pub const PENDING_MG: &[&str] = &[
    "storeUpgrade.refused.detail.snapshotMissing",
    "storeUpgrade.refused.changedByUpgrade",
];

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
    }
}

/// Marks copy that MG has not written yet; a test lists every use.
pub const PLACEHOLDER: &str = "[MG placeholder] ";

const CONFIRM_TITLE: &str = "Restore wallet data from before the upgrade?";
const CONFIRM_TEXT: &str = "Dash Wallet restarts and puts back the wallet data it saved before the upgrade, so the wallet won't show anything recorded since then. Until the wallet has finished syncing, coins and Shielded notes you've already spent may show as unspent. The wallet data recorded since the upgrade is kept in the upgrade-backups folder inside {folder}. Don't delete it: it may hold the only record of a recent payment.

This version of Dash Wallet pauses sending until syncing finishes. An earlier version doesn't. If you open the wallet with one, don't send anything until it has finished syncing. An earlier version may offer coins this version is holding back, and a payment that uses them can fail.

If Dash Wallet closes before the restore finishes, start this version again before you open an earlier one.";

const RESTORE_REFUSED_TITLE: &str = "The earlier wallet data couldn't be restored";
const RESTORE_REFUSED_TEXT: &str = "Dash Wallet couldn't use the wallet data it saved before the upgrade, so nothing was changed. The wallet is using the same data as before you chose to restore.";

const INCOMPLETE_TITLE: &str = "Dash Wallet couldn't finish restoring your wallet data";
const INCOMPLETE_TEXT: &str = "Dash Wallet stopped partway through putting back the wallet data it saved before the upgrade, so it hasn't opened the wallet data in {folder}. The data saved before the upgrade and the data it was replacing are both still in the upgrade-backups folder inside {folder}: don't delete anything there. Make sure the disk has free space and that Dash Wallet can write to {folder}, then restart Dash Wallet to finish. Don't open an earlier version of Dash Wallet until this one has finished. If it still can't finish, choose Report a problem.";

const NO_SPACE_TITLE: &str = "Not enough disk space to restore";
const NO_SPACE_TEXT: &str = "Dash Wallet needs more free disk space to restore your wallet data from before the upgrade, so nothing was changed. Free up space on the disk that holds {folder}, then choose to restore again.";

const NOT_STARTED_TITLE: &str =
    "[MG placeholder] Dash Wallet couldn't start restoring your wallet data";
const NOT_STARTED_TEXT: &str = "[MG placeholder] Dash Wallet couldn't start putting back the wallet data it saved before the upgrade, so nothing was changed. Restart Dash Wallet to try again.";

const KEEP_CURRENT_TITLE: &str =
    "[MG placeholder] Dash Wallet couldn't finish restoring your wallet data";
const KEEP_CURRENT_TEXT: &str = "[MG placeholder] Another version of Dash Wallet changed the wallet data in {folder} after the earlier data was put back. You can keep the wallet data as it is now. Every set of wallet data is kept in the upgrade-backups folder inside {folder}: don't delete anything there.";

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
        ("storeUpgrade.restore.refused.title", RESTORE_REFUSED_TITLE),
        ("storeUpgrade.restore.refused.text", RESTORE_REFUSED_TEXT),
        vec![button("storeUpgrade.restore.refused.ok", "OK")],
    )
}

/// `storeUpgrade.restore.incomplete.*`, the same for every cause; the cause
/// stays in the error for the report (MG (q)).
pub fn restore_incomplete(_cause: IncompleteCause) -> ScreenCopy {
    ScreenCopy {
        hint: Some((ISSUES_HINT.key, ISSUES_HINT.text)),
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
        hint: Some((ISSUES_HINT.key, ISSUES_HINT.text)),
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

/// The restore flow's screen for an error from this crate, if it has one.
/// Every error has at most one of this and [`screen_for`].
pub fn restore_screen_for(e: &StoreUpgradeError) -> Option<ScreenCopy> {
    Some(match e {
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

/// Every restore screen this module renders, for catalog checks.
pub fn all_screens() -> Vec<ScreenCopy> {
    use crate::restore::RefusalCause;
    let mut v = vec![
        restore_confirm(),
        restore_refused(RefusalCause::MissingBackup),
        restore_incomplete(IncompleteCause::Other),
        restore_no_space(),
        restore_not_started(IncompleteCause::Other),
        restore_keep_current(),
    ];
    v.dedup();
    v
}

/// `storeUpgrade.restore.foreign.*` (MG, Q93 / Q92 v2.1 copy): the live
/// wallet fails the restore's lineage test. Copy data for the restore flow;
/// besides Keep the screen has Report a problem (with [`ISSUES_HINT`]) and
/// Quit, and Keep is not the default focus.
pub mod restore_foreign {
    use super::Line;

    pub const TITLE: Line = super::line(
        "storeUpgrade.restore.foreign.title",
        "These wallet files don't match the restore",
    );
    pub const TEXT: Line = super::line(
        "storeUpgrade.restore.foreign.text",
        "Dash Wallet was restoring your wallet data from before the upgrade, but the wallet files now in {folder} don't match that data, so it stopped without changing them. Another version of Dash Wallet or another program may have replaced them.",
    );
    pub const KEEP: Line = super::line(
        "storeUpgrade.restore.foreign.keep",
        "Keep the current files",
    );
    pub const KEEP_MEANING: Line = super::line(
        "storeUpgrade.restore.foreign.keepMeaning",
        "This stops the restore and opens the files in {folder} as they are now. Nothing is deleted: the wallet data saved before the upgrade, and the copy taken when the restore started, stay in the upgrade-backups folder.",
    );
}

/// The folder name the copy names: the one the snapshot is written under.
pub const COPY_FOLDER_NAME: &str = UPGRADE_BACKUPS_DIR;
