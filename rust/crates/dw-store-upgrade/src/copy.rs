//! MG's store-upgrade copy, verbatim, as a model the screen renders.
//! `{folder}` is the network directory the screen names; "upgrade-backups"
//! is the literal folder name (M3-UX (e)).
//!
//! The refusal at open (`storeUpgrade.refused.*`, MG's final cantOpen
//! family, sha256 8c2c34db…0da2) is title + "{intro}: {detail} {unchanged}
//! {wayOut}". The detail follows the refusal's cause; the way out follows
//! the file's proved state, never the cause alone, and an unproved state
//! never offers the earlier version.

use std::path::Path;

use crate::error::StoreUpgradeError;
use crate::snapshot::{STORE_FILES, UPGRADE_BACKUPS_DIR};

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

/// `settings.links.issuesHint`, shown under every "Report a problem".
pub const ISSUES_HINT: Line = line(
    "settings.links.issuesHint",
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
    pub const ALL: [Self; 9] = [
        Self::ForkUnsupported,
        Self::ShapeMismatch,
        Self::RehearsalFailed,
        Self::DevBuild,
        Self::TooNew,
        Self::HistoryMismatch,
        Self::ContactsTable,
        Self::SelfCheck,
        Self::Unexpected,
    ];

    /// The cause the refusal screen shows for `e`, or `None` when `e` gets
    /// no refusal screen: a retryable [`StoreUpgradeError::Io`], a pending
    /// restore (the restore screens own it), or an error after which the
    /// files may have changed, where "didn't change anything" can't be said.
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
            E::RetiredPoolWithoutAdopter { .. } => Self::ContactsTable,
            E::P9aBroken { .. } => Self::SelfCheck,
            E::Io { .. } | E::RestorePending { .. } => return None,
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

/// What the engine proved about the refused file. `None` in a field means
/// not proved, which never offers the earlier version (MG (A)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProvedState {
    /// A new build migrated it (refinery's `MAX(version)` past V018) or
    /// part-upgraded it, so no earlier version opens it ([`migrated`]).
    pub migrated: Option<bool>,
    pub backup: Option<BackupState>,
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
        return Some(true);
    }
    if crate::sql::has_table(&conn, crate::fork::FORK_HISTORY).ok()? {
        if !crate::sql::has_table(&conn, crate::fork::CONTACTS).ok()? {
            return None;
        }
        if !crate::sql::has_column(&conn, crate::fork::CONTACTS, crate::fork::EXTERNAL_REF).ok()? {
            return Some(true);
        }
    }
    let logged = ["-wal", "-journal"].iter().any(|suffix| {
        std::fs::metadata(network_dir.join(format!("{}{suffix}", STORE_FILES[0])))
            .is_ok_and(|m| m.len() > 0)
    });
    (!logged).then_some(false)
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
    pub unchanged: Line,
    pub way_out_line: Line,
    pub buttons: Vec<Button>,
    /// `settings.links.issuesHint`, under the buttons with Report a problem.
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
            unchanged: REFUSED_UNCHANGED,
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
/// `state`: [`migrated`] for the file, the snapshot's verification for the
/// backup.
pub fn screen_for(e: &StoreUpgradeError, state: ProvedState) -> Option<RefusedScreen> {
    RefusedCause::of(e).map(|cause| RefusedScreen::new(cause, state))
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
