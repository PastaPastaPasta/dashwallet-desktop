//! Process-wide app state, managed by Tauri, and the handle tables that let
//! engine objects (a `TxDraft`, its `PreparedTx`s) outlive one command.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;

use dw_engine::platform::DashPay;
use dw_engine::{Engine, EngineConfig, NetworkSession, WalletId};
use dw_vault::VaultConfig;
use tauri::{AppHandle, Runtime};
use tokio::sync::Mutex;

use crate::bridge::TauriSink;
use crate::error::{AppCode, AppError};
use crate::launch::LaunchOptions;

pub struct AppState {
    pub launch: LaunchOptions,
    /// Taken at the top of `main`, for the cold-start measurement (U5).
    pub started: Instant,
    /// `None` in fixture mode: the renderer brings its own data.
    pub engine: Option<Engine>,
    /// The network the window shows (D-row "one active session"). Session
    /// commands act on it; `open_network` and `close_network` change it.
    session: Mutex<Option<Arc<NetworkSession>>>,
    /// Ends the app (`AppHandle::exit`); `selftest_report` calls it.
    exit: Box<dyn Fn(i32) + Send + Sync>,
    pub drafts: HandleTable<Arc<dw_engine::TxDraft>>,
    /// A prepared transaction with the id of the draft that made it.
    pub prepared: HandleTable<(u64, Arc<dw_engine::PreparedTx>)>,
}

impl AppState {
    pub fn new<R: Runtime>(
        app: &AppHandle<R>,
        launch: LaunchOptions,
        started: Instant,
        default_data_dir: impl FnOnce() -> Result<PathBuf, AppError>,
    ) -> Result<Self, AppError> {
        let engine = if launch.fixture {
            None
        } else {
            let data_root = match &launch.data_dir {
                Some(dir) => dir.clone(),
                None => default_data_dir()?,
            };
            Some(Engine::new(
                EngineConfig {
                    data_root,
                    worker_threads: None,
                    vault: VaultConfig::default(),
                },
                Arc::new(TauriSink::new(app.clone())?),
            )?)
        };
        let handle = app.clone();
        Ok(AppState {
            launch,
            started,
            engine,
            session: Mutex::new(None),
            exit: Box::new(move |code| handle.exit(code)),
            drafts: HandleTable::default(),
            prepared: HandleTable::default(),
        })
    }

    pub fn engine(&self) -> Result<&Engine, AppError> {
        self.engine.as_ref().ok_or_else(|| {
            AppError::app(
                AppCode::FixtureMode,
                "the app runs on fixture data; no engine",
            )
        })
    }

    /// The active session; `app.network_not_open` before `open_network`.
    pub async fn session(&self) -> Result<Arc<NetworkSession>, AppError> {
        self.session
            .lock()
            .await
            .clone()
            .ok_or_else(|| AppError::app(AppCode::NetworkNotOpen, "call open_network first"))
    }

    /// The slot `open_network` and `close_network` hold while they change
    /// the active session, so two of them never interleave.
    pub(crate) async fn session_slot(
        &self,
    ) -> tokio::sync::MutexGuard<'_, Option<Arc<NetworkSession>>> {
        self.session.lock().await
    }

    /// The DashPay facade of one wallet of the active session.
    pub async fn dashpay(&self, wallet_id: &str) -> Result<Arc<DashPay>, AppError> {
        let id = parse_wallet_id(wallet_id)?;
        Ok(self.session().await?.dashpay(id))
    }

    /// The `--selftest` report path; the self-test commands refuse to run without it.
    pub fn selftest_path(&self) -> Result<&Path, AppError> {
        self.launch.selftest.as_deref().ok_or_else(|| {
            AppError::app(
                AppCode::NotSelftest,
                "the app was not launched with --selftest",
            )
        })
    }

    pub fn exit(&self, code: i32) {
        (self.exit)(code)
    }

    /// Drops every handle: they belong to the session that is closing.
    pub(crate) fn clear_handles(&self) {
        self.drafts.clear();
        self.prepared.clear();
    }
}

/// Wallet ids cross the IPC boundary as lower-case hex.
pub fn parse_wallet_id(s: &str) -> Result<WalletId, AppError> {
    s.parse().map_err(AppError::from)
}

/// Engine objects the renderer refers to by a number, because they hold
/// state between calls (a draft's recipients, a signed transaction) and
/// cannot cross the IPC boundary themselves.
///
/// Ids are never reused within a process. The table is capped: a renderer
/// that leaks handles gets `app.too_many_handles` instead of growing the
/// process, and closing the session drops them all.
pub struct HandleTable<T> {
    inner: StdMutex<Handles<T>>,
}

struct Handles<T> {
    next: u64,
    items: HashMap<u64, T>,
}

impl<T> Default for HandleTable<T> {
    fn default() -> Self {
        HandleTable {
            inner: StdMutex::new(Handles {
                next: 1,
                items: HashMap::new(),
            }),
        }
    }
}

impl<T: Clone> HandleTable<T> {
    /// Live handles per table.
    pub const MAX: usize = 64;

    fn lock(&self) -> std::sync::MutexGuard<'_, Handles<T>> {
        // A panic while holding the lock leaves the map itself consistent.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn insert(&self, item: T) -> Result<u64, AppError> {
        let mut h = self.lock();
        if h.items.len() >= Self::MAX {
            return Err(AppError::app(
                AppCode::TooManyHandles,
                format!("more than {} open handles; close some first", Self::MAX),
            ));
        }
        let id = h.next;
        h.next += 1;
        h.items.insert(id, item);
        Ok(id)
    }

    pub fn get(&self, id: u64) -> Result<T, AppError> {
        self.lock()
            .items
            .get(&id)
            .cloned()
            .ok_or_else(|| AppError::app(AppCode::UnknownHandle, format!("no handle {id}")))
    }

    pub fn remove(&self, id: u64) -> Option<T> {
        self.lock().items.remove(&id)
    }

    /// Removes every item `drop_it` selects.
    pub fn remove_where(&self, mut drop_it: impl FnMut(&T) -> bool) {
        self.lock().items.retain(|_, item| !drop_it(item));
    }

    pub fn clear(&self) {
        self.lock().items.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_are_capped_and_never_reused() {
        let t = HandleTable::<u32>::default();
        let first = t.insert(7).unwrap();
        assert_eq!(t.get(first).unwrap(), 7);
        assert_eq!(t.remove(first), Some(7));
        assert_eq!(t.get(first).unwrap_err().code_str(), "app.unknown_handle");
        let ids: Vec<_> = (0..HandleTable::<u32>::MAX)
            .map(|i| t.insert(i as u32).unwrap())
            .collect();
        assert!(!ids.contains(&first));
        assert_eq!(t.insert(0).unwrap_err().code_str(), "app.too_many_handles");
        t.remove_where(|v| *v % 2 == 0);
        assert_eq!(t.lock().items.len(), HandleTable::<u32>::MAX / 2);
        t.clear();
        assert!(t.insert(1).is_ok());
    }
}
