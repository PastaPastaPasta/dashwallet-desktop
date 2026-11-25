//! The persister open through this crate, and the process gate restore
//! checks (Q82-G1 F4-a/F4-b).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use platform_wallet_storage::{SqlitePersister, SqlitePersisterConfig, WalletStorageError};

use crate::error::{Result, StoreUpgradeError};

/// Proof that `SqlitePersister::open` succeeded on a network directory's
/// `wallet.sqlite` in this process, and that its store is held while this
/// value lives.
///
/// Sealed: the only constructor is [`PersisterOpened::open`], which opens the
/// persister itself, so a token can't exist without a successful open.
/// Keep it next to the persister and drop it with the persister.
#[derive(Debug)]
pub struct PersisterOpened {
    network_dir: PathBuf,
}

fn held_dirs() -> &'static Mutex<HashSet<PathBuf>> {
    static HELD: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(HashSet::new()))
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

/// Whether a [`PersisterOpened`] for `network_dir` is alive.
pub(crate) fn is_held(network_dir: &Path) -> bool {
    held_dirs()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .contains(&key(network_dir))
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
    /// `wallet.sqlite`) and mint the token. Refuses while a restore is
    /// requested or part-done there. Upstream's `AlreadyOpen` comes back as
    /// [`StoreUpgradeError::StoreHeld`], never as an empty store (F4-d).
    pub fn open(config: SqlitePersisterConfig) -> Result<(SqlitePersister, Self)> {
        let network_dir = config
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        set_opened();
        crate::restore::refuse_if_restore_pending(&network_dir)?;
        let persister =
            SqlitePersister::open(config).map_err(|e| map_open_error(e, &network_dir))?;
        held_dirs()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key(&network_dir));
        Ok((persister, Self { network_dir }))
    }

    pub fn network_dir(&self) -> &Path {
        &self.network_dir
    }
}

impl Drop for PersisterOpened {
    fn drop(&mut self) {
        held_dirs()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key(&self.network_dir));
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
