use std::collections::BTreeSet;
use std::path::PathBuf;

/// Every way the store upgrade stops. [`StoreUpgradeError::file_unchanged`]
/// says whether the wallet files were left exactly as found.
#[derive(Debug, thiserror::Error)]
pub enum StoreUpgradeError {
    /// The fork history holds a version set no published build wrote, or one
    /// whose adopting migration this build lacks (Q78 R4). File unchanged.
    #[error("fork store versions {versions:?} are not supported by this build")]
    ForkStoreUnsupported { versions: BTreeSet<u32> },

    /// A fork object differs from the fork's exact definition, or another
    /// object refers to the column R1 drops. File unchanged.
    #[error("fork store object {object} does not have the expected shape")]
    ForkStoreShapeMismatch { object: String },

    /// `core_address_pool_retired` holds rows and this build's P9a probe read
    /// Absent (Q78-A R1 amendment §2). File unchanged.
    #[error("{rows} retired contact-pool rows and this build has no P9a")]
    RetiredPoolWithoutAdopter { rows: u64 },

    /// The P9a probe read Broken: every wallet open is refused (Q78-A A4).
    /// File unchanged.
    #[error("this build's P9a support is broken: {detail}")]
    P9aBroken { detail: String },

    /// The upgrade, rehearsed on a private copy (R1's drop, then the upstream
    /// open under `LoadPolicy::Strict` with the caller's settings), failed on
    /// the data: the real file would be left part-upgraded. A failure of the
    /// machine (disk full, I/O, the backup directory) is [`Self::Io`]
    /// instead, which is retried and has no can't-open screen. File unchanged.
    #[error("the upgrade fails on a copy of this wallet file: {detail}")]
    UpgradeRehearsalFailed { detail: String },

    /// Fork V1 recorded as refinery version 19, before the fork history table
    /// existed. File unchanged.
    #[error("the file records the fork migration in upstream's history (pre-history shape)")]
    PreHistoryFork,

    /// Refinery `MAX(version)` above this build's embedded migrations. File
    /// unchanged.
    #[error("schema version {found} is newer than this build's {supported}")]
    SchemaTooNew { found: i64, supported: i64 },

    /// An applied refinery row differs from this build's embedded migration.
    /// File unchanged.
    #[error("refinery history version {version}: {detail}")]
    SchemaHistoryMismatch { version: i64, detail: String },

    /// The snapshot handed to the reconciler or restore is missing, not
    /// complete, or not this directory's. File unchanged.
    #[error("upgrade snapshot {dir}: {detail}")]
    SnapshotInvalid { dir: PathBuf, detail: String },

    /// Taking the pre-upgrade snapshot failed. The wallet files are unchanged;
    /// the incomplete snapshot directory is redone on the next start.
    #[error("upgrade snapshot failed: {detail}")]
    SnapshotFailed { detail: String },

    /// A restore is requested or part-done in {dir}: no store may open
    /// there until the next start runs it.
    #[error("a restore is pending in {dir}")]
    RestorePending { dir: PathBuf },

    /// The requested restore was refused before anything moved
    /// (`storeUpgrade.restore.refused.*`). The request was removed so the
    /// refusal shows once (MG (k)); every wallet and backup file is unchanged.
    #[error("restore refused: {cause}")]
    RestoreRefused { cause: crate::restore::RefusalCause },

    /// Not enough space for the copies a restore makes before its journal
    /// (`storeUpgrade.restore.noSpace.*`). This attempt's copies were removed
    /// (or marked safe to delete) and the request cleared (MG (s)). Nothing
    /// was changed. `needed` is the extra space the copies take.
    #[error("not enough disk space to restore (needs {needed:?} bytes)")]
    RestoreNoSpace { needed: Option<u64> },

    /// A restore stopped. Before its journal (`journal_written` false) nothing
    /// changed and the request stays (`storeUpgrade.restore.notStarted.*`);
    /// after it the stores may be part-swapped and no store may open until a
    /// restart finishes it (`storeUpgrade.restore.incomplete.*`).
    #[error("restore stopped ({cause}): {detail}")]
    RestoreIncomplete {
        cause: crate::restore::IncompleteCause,
        journal_written: bool,
        detail: String,
    },

    /// A restore was asked for after a store opened in this process
    /// (Q82-G1 F4-a, Q86 (f)). Nothing changed; the request stays.
    #[error("restore refused: a wallet store has already opened in this process")]
    RestoreAfterOpen,

    /// The store is held open in this process (upstream `AlreadyOpen`, a
    /// live [`crate::PersisterOpened`], or the `app.sqlite` lock). Never read
    /// as "no wallet" (Q82-G1 F4-d). Nothing changed.
    #[error("the wallet store at {path} is open")]
    StoreHeld { path: PathBuf },

    /// An upgraded store (R1's drop committed, or upstream migrated and the
    /// fork history not yet retired) and no before-upgrade snapshot this
    /// upgrade can keep: none at all, none that verifies, or none of this
    /// wallet from before the upgrade
    /// (`storeUpgrade.refused.detail.snapshotMissing`). The file stays as
    /// the earlier start left it; nothing more is written.
    #[error("the upgrade is part-done and its snapshot is {state:?} in {dir}")]
    UpgradeSnapshotMissing {
        state: crate::snapshot::MissingSnapshot,
        dir: PathBuf,
    },

    /// `wallet.sqlite` is a symbolic link (unix only, as upstream) or is
    /// owned by another user: the upstream open refuses the first and can't
    /// make the second owner-only, and the copy the upgrade is rehearsed on
    /// has neither. Refused before R1's drop. Its screen is the
    /// unexpected-state refusal. Nothing changed.
    #[error("wallet.sqlite {detail}")]
    WalletFileUnsupported { detail: String },

    /// [`crate::PersisterOpened::open`] was asked for a load policy other
    /// than `Strict` (Q78 R6). Nothing was opened.
    #[error("the wallet store opens only with LoadPolicy::Strict, not {policy}")]
    NonStrictOpen { policy: String },

    /// The `SqlitePersisterConfig` can't carry the upgrade: no auto-backup
    /// directory, or not the config [`crate::reconcile_fork_store`]
    /// rehearsed with. A caller bug, not the user's data. Nothing changed.
    #[error("the wallet store config is not supported: {detail}")]
    OpenConfigUnsupported { detail: String },

    /// [`crate::PersisterOpened::open`] ran before this build's probe
    /// ([`crate::Adopters::for_build`]). Nothing was opened.
    #[error("the platform-wallet-storage probe has not run in this process")]
    ProbeNotRun,

    /// `SqlitePersister::open` failed. Its grouped migration rolls back, but
    /// an open can fail after that commit, so this does not claim the file
    /// unchanged.
    #[error("open the wallet store in {dir}: {source}")]
    PersisterOpen {
        dir: PathBuf,
        #[source]
        source: Box<platform_wallet_storage::WalletStorageError>,
    },

    /// SQLite failed. `unchanged` is false only when a write may have
    /// committed.
    #[error("{context}: {source}")]
    Sqlite {
        context: &'static str,
        unchanged: bool,
        #[source]
        source: rusqlite::Error,
    },

    /// The file system failed. `unchanged` as for [`Self::Sqlite`].
    #[error("{context}: {source}")]
    Io {
        context: &'static str,
        unchanged: bool,
        #[source]
        source: std::io::Error,
    },

    #[cfg(test)]
    #[error("injected fault at {0}")]
    InjectedFault(&'static str),
}

impl StoreUpgradeError {
    /// Whether the wallet files were left exactly as found.
    pub fn file_unchanged(&self) -> bool {
        match self {
            Self::ForkStoreUnsupported { .. }
            | Self::ForkStoreShapeMismatch { .. }
            | Self::RetiredPoolWithoutAdopter { .. }
            | Self::P9aBroken { .. }
            | Self::UpgradeRehearsalFailed { .. }
            | Self::PreHistoryFork
            | Self::SchemaTooNew { .. }
            | Self::SchemaHistoryMismatch { .. }
            | Self::NonStrictOpen { .. }
            | Self::OpenConfigUnsupported { .. }
            | Self::ProbeNotRun
            | Self::SnapshotInvalid { .. }
            | Self::SnapshotFailed { .. }
            | Self::UpgradeSnapshotMissing { .. }
            | Self::WalletFileUnsupported { .. }
            | Self::RestorePending { .. }
            | Self::RestoreRefused { .. }
            | Self::RestoreNoSpace { .. }
            | Self::RestoreAfterOpen
            | Self::StoreHeld { .. } => true,
            Self::RestoreIncomplete {
                journal_written, ..
            } => !journal_written,
            Self::PersisterOpen { .. } => false,
            Self::Sqlite { unchanged, .. } | Self::Io { unchanged, .. } => *unchanged,
            #[cfg(test)]
            Self::InjectedFault(_) => true,
        }
    }

    /// Whether this build refuses this data as it is, leaving it unchanged.
    /// Every refusal has a screen ([`crate::copy::screen_for`], Q98 98-2).
    pub fn is_refusal(&self) -> bool {
        matches!(
            self,
            Self::ForkStoreUnsupported { .. }
                | Self::ForkStoreShapeMismatch { .. }
                | Self::RetiredPoolWithoutAdopter { .. }
                | Self::P9aBroken { .. }
                | Self::UpgradeRehearsalFailed { .. }
                | Self::PreHistoryFork
                | Self::SchemaTooNew { .. }
                | Self::SchemaHistoryMismatch { .. }
                | Self::UpgradeSnapshotMissing { .. }
                | Self::WalletFileUnsupported { .. }
        )
    }

    pub(crate) fn sqlite(
        context: &'static str,
        unchanged: bool,
    ) -> impl Fn(rusqlite::Error) -> Self {
        move |source| Self::Sqlite {
            context,
            unchanged,
            source,
        }
    }

    pub(crate) fn io(context: &'static str, unchanged: bool) -> impl Fn(std::io::Error) -> Self {
        move |source| Self::Io {
            context,
            unchanged,
            source,
        }
    }
}

pub type Result<T, E = StoreUpgradeError> = std::result::Result<T, E>;
