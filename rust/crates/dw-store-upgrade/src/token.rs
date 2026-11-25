//! The persister open through this crate, and the process gate restore
//! checks (Q82-G1 F4-a/F4-b).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use platform_wallet_storage::{
    LoadPolicy, SqlitePersister, SqlitePersisterConfig, WalletStorageError,
};

use crate::error::{Result, StoreUpgradeError};

/// Proof that `SqlitePersister::open` succeeded on a network directory's
/// `wallet.sqlite` in this process.
///
/// Sealed: the only constructor is [`PersisterOpened::open`], which opens the
/// persister itself, so a token can't exist without a successful open. The
/// store counts as held while any clone of the returned `Arc` lives, whether
/// or not the token does.
#[derive(Debug)]
pub struct PersisterOpened {
    network_dir: PathBuf,
}

fn held_dirs() -> &'static Mutex<HashMap<PathBuf, Weak<SqlitePersister>>> {
    static HELD: OnceLock<Mutex<HashMap<PathBuf, Weak<SqlitePersister>>>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(not(test))]
static OPENED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    static OPENED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn set_opened() {
    #[cfg(not(test))]
    OPENED.store(true, std::sync::atomic::Ordering::SeqCst);
    #[cfg(test)]
    OPENED.with(|o| o.set(true));
}

/// Whether any store has opened, or begun opening, in this process.
pub fn stores_opened_in_process() -> bool {
    #[cfg(not(test))]
    return OPENED.load(std::sync::atomic::Ordering::SeqCst);
    #[cfg(test)]
    return OPENED.with(|o| o.get());
}

#[cfg(test)]
pub(crate) fn reset_opened_for_test() {
    OPENED.with(|o| o.set(false));
}

/// Record that this process is about to open a network directory's stores
/// (vault, `app.sqlite`, the persister). From here on [`crate::restore`]
/// refuses: a restore runs only before any store opens in the process.
pub fn note_stores_opening() {
    set_opened();
}

fn key(network_dir: &Path) -> PathBuf {
    network_dir
        .canonicalize()
        .unwrap_or_else(|_| network_dir.to_path_buf())
}

/// Whether a persister opened through [`PersisterOpened::open`] for
/// `network_dir` is still alive.
pub(crate) fn is_held(network_dir: &Path) -> bool {
    held_dirs()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&key(network_dir))
        .is_some_and(|w| w.strong_count() > 0)
}

#[cfg(test)]
thread_local! {
    pub(crate) static PROBE_FOR_OPEN: std::cell::RefCell<Option<Option<crate::adopters::P9aStatus>>> =
        const { std::cell::RefCell::new(None) };
}

fn probe_for_open() -> Option<crate::adopters::P9aStatus> {
    #[cfg(test)]
    if let Some(forced) = PROBE_FOR_OPEN.with(|p| p.borrow().clone()) {
        return forced;
    }
    #[cfg(test)]
    crate::tests::build();
    crate::adopters::Adopters::cached().map(|a| a.p9a_status().clone())
}

fn refuse_unless_probed() -> Result<()> {
    match probe_for_open() {
        None => Err(StoreUpgradeError::ProbeNotRun),
        Some(crate::adopters::P9aStatus::Broken { detail }) => {
            Err(StoreUpgradeError::P9aBroken { detail })
        }
        Some(_) => Ok(()),
    }
}

pub(crate) fn refuse_if_held(network_dir: &Path) -> Result<()> {
    if is_held(network_dir) {
        return Err(StoreUpgradeError::StoreHeld {
            path: network_dir.to_path_buf(),
        });
    }
    Ok(())
}

impl PersisterOpened {
    /// Open the persister for `config.path` (a network directory's
    /// `wallet.sqlite`) and mint the token. Refuses, opening nothing, when
    /// the load policy is not `Strict` (Q78 R6), when this build's probe has
    /// not run or read Broken (Q78-A A4), and while a restore is requested or
    /// part-done there. Upstream's `AlreadyOpen` comes back as
    /// [`StoreUpgradeError::StoreHeld`], never as an empty store (F4-d).
    pub fn open(config: SqlitePersisterConfig) -> Result<(Arc<SqlitePersister>, Self)> {
        let network_dir = crate::precheck::parent_dir(&config.path)?.to_path_buf();
        set_opened();
        if config.load_policy != LoadPolicy::Strict {
            return Err(StoreUpgradeError::NonStrictOpen {
                policy: format!("{:?}", config.load_policy),
            });
        }
        refuse_unless_probed()?;
        crate::restore::refuse_if_restore_pending(&network_dir)?;
        let persister =
            Arc::new(SqlitePersister::open(config).map_err(|e| map_open_error(e, &network_dir))?);
        held_dirs()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key(&network_dir), Arc::downgrade(&persister));
        Ok((persister, Self { network_dir }))
    }

    pub fn network_dir(&self) -> &Path {
        &self.network_dir
    }
}

pub(crate) fn map_open_error(e: WalletStorageError, network_dir: &Path) -> StoreUpgradeError {
    match e {
        WalletStorageError::AlreadyOpen { path } => StoreUpgradeError::StoreHeld { path },
        other => StoreUpgradeError::PersisterOpen {
            dir: network_dir.to_path_buf(),
            source: Box::new(other),
        },
    }
}
