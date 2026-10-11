//! Process-wide app state, managed by Tauri, and the handle tables that let
//! engine objects (a `TxDraft`, its `PreparedTx`s) outlive one command.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;

use dw_engine::platform::DashPay;
use dw_engine::{Engine, EngineConfig, NetworkSession, PreparedTx, TxDraft, WalletId};
use dw_vault::VaultConfig;
use tauri::{AppHandle, Runtime};
use tokio::sync::Mutex;

use crate::bridge::TauriSink;
use crate::error::{AppCode, AppError, EngineCode};
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
    /// The drafts and prepared transactions of the active session.
    pub sends: SendHandles,
}

impl AppState {
    /// `vault` is the production `VaultConfig::default()` except in tests
    /// (an in-memory OS store, cheap KDF parameters).
    pub fn new<R: Runtime>(
        app: &AppHandle<R>,
        launch: LaunchOptions,
        started: Instant,
        vault: VaultConfig,
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
                    vault,
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
            sends: SendHandles::default(),
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

    /// The active session; `network_not_open` before `open_network`.
    pub async fn session(&self) -> Result<Arc<NetworkSession>, AppError> {
        Ok(self.session_and_epoch().await?.0)
    }

    /// The active session with the handle epoch it belongs to, read
    /// together: `open_network` and `close_network` clear the handles while
    /// they hold the session, so a handle made from this session can be
    /// stored only under this epoch.
    pub async fn session_and_epoch(&self) -> Result<(Arc<NetworkSession>, u64), AppError> {
        let slot = self.session.lock().await;
        let session = slot
            .clone()
            .ok_or_else(|| AppError::new(EngineCode::NetworkNotOpen, "call open_network first"))?;
        Ok((session, self.sends.epoch()))
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

    /// Developer-only commands (the trusted-quorums toggle, m4 contract)
    /// refuse in release builds.
    pub fn developer_build(&self) -> Result<(), AppError> {
        if cfg!(debug_assertions) {
            Ok(())
        } else {
            Err(AppError::app(
                AppCode::NotDeveloperBuild,
                "a developer-only command in a release build",
            ))
        }
    }

    pub fn exit(&self, code: i32) {
        (self.exit)(code)
    }

    /// Drops every handle: they belong to the session that is closing.
    pub(crate) fn clear_handles(&self) {
        self.sends.clear();
    }
}

/// Wallet ids cross the IPC boundary as lower-case hex.
pub fn parse_wallet_id(s: &str) -> Result<WalletId, AppError> {
    s.parse().map_err(AppError::from)
}

/// Engine objects the renderer refers to by a number, because they hold
/// state between calls (a draft's recipients, a signed transaction) and
/// cannot cross the IPC boundary themselves: `TxDraft`s, and `PreparedTx`s
/// with the draft that made each.
///
/// Ids are never reused within a process. Each kind is capped: a renderer
/// that leaks handles gets `app.too_many_handles` instead of growing the
/// process. Closing the session drops them all and starts a new epoch. One
/// lock covers both kinds, so a prepared transaction is stored only while
/// its draft is open, and a draft only in the epoch of its session; one
/// that arrives late is dropped (a pending prepared transaction then
/// releases its inputs).
#[derive(Default)]
pub struct SendHandles {
    inner: StdMutex<Sends>,
}

#[derive(Default)]
struct Sends {
    epoch: u64,
    /// The last id given out.
    last: u64,
    drafts: HashMap<u64, Arc<TxDraft>>,
    /// Each with its draft, and whether a broadcast of it was attempted.
    prepared: HashMap<u64, Prepared>,
}

struct Prepared {
    draft: u64,
    tx: Arc<PreparedTx>,
    broadcast: bool,
}

/// `app.too_many_handles` when `open` handles of a kind are the cap.
fn full(open: usize) -> Result<(), AppError> {
    if open >= SendHandles::MAX {
        return Err(AppError::new(
            AppCode::TooManyHandles,
            format!(
                "more than {} open handles; close some first",
                SendHandles::MAX
            ),
        ));
    }
    Ok(())
}

impl Sends {
    fn next_id(&mut self, open: usize) -> Result<u64, AppError> {
        full(open)?;
        self.last += 1;
        Ok(self.last)
    }
}

fn unknown(id: u64) -> AppError {
    AppError::new(AppCode::UnknownHandle, format!("no handle {id}"))
}

impl SendHandles {
    /// Live handles per kind.
    pub const MAX: usize = 64;

    fn lock(&self) -> std::sync::MutexGuard<'_, Sends> {
        // A panic while holding the lock leaves the maps themselves consistent.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn epoch(&self) -> u64 {
        self.lock().epoch
    }

    /// Stores a draft made from the session of `epoch`.
    pub fn insert_draft(&self, epoch: u64, draft: Arc<TxDraft>) -> Result<u64, AppError> {
        let mut s = self.lock();
        if s.epoch != epoch {
            return Err(AppError::new(
                EngineCode::NetworkNotOpen,
                "the network closed while the draft was made",
            ));
        }
        let open = s.drafts.len();
        let id = s.next_id(open)?;
        s.drafts.insert(id, draft);
        Ok(id)
    }

    pub fn draft(&self, id: u64) -> Result<Arc<TxDraft>, AppError> {
        self.lock()
            .drafts
            .get(&id)
            .cloned()
            .ok_or_else(|| unknown(id))
    }

    /// Drops a draft and its prepared transactions; `false` when it was not open.
    pub fn close_draft(&self, id: u64) -> bool {
        let mut s = self.lock();
        s.prepared.retain(|_, p| p.draft != id);
        s.drafts.remove(&id).is_some()
    }

    /// Fails like `insert_prepared` would on a full table: checked before a
    /// prepare redeems its grant.
    pub fn room_for_prepared(&self) -> Result<(), AppError> {
        full(self.lock().prepared.len())
    }

    /// Stores a transaction prepared from `draft`, if the draft is still open.
    pub fn insert_prepared(&self, draft: u64, tx: Arc<PreparedTx>) -> Result<u64, AppError> {
        let mut s = self.lock();
        if !s.drafts.contains_key(&draft) {
            return Err(AppError::new(
                AppCode::UnknownHandle,
                format!("draft {draft} was closed while it was prepared"),
            ));
        }
        let open = s.prepared.len();
        let id = s.next_id(open)?;
        s.prepared.insert(
            id,
            Prepared {
                draft,
                tx,
                broadcast: false,
            },
        );
        Ok(id)
    }

    /// A prepared transaction with the draft that made it.
    pub fn prepared(&self, id: u64) -> Result<(u64, Arc<PreparedTx>), AppError> {
        let s = self.lock();
        let p = s.prepared.get(&id).ok_or_else(|| unknown(id))?;
        Ok((p.draft, Arc::clone(&p.tx)))
    }

    /// Marks a broadcast of `id` as attempted; `true` for the first one.
    pub fn start_broadcast(&self, id: u64) -> bool {
        self.lock()
            .prepared
            .get_mut(&id)
            .is_some_and(|p| !std::mem::replace(&mut p.broadcast, true))
    }

    pub fn remove_prepared(&self, id: u64) {
        self.lock().prepared.remove(&id);
    }

    fn clear(&self) {
        let mut s = self.lock();
        s.epoch += 1;
        s.drafts.clear();
        s.prepared.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_are_capped_and_never_reused() {
        // Exercised with real drafts over IPC (`ipc_tests`); here only the
        // id and cap rules, which need no engine.
        let mut s = Sends::default();
        let ids: Vec<u64> = (0..SendHandles::MAX)
            .map(|n| s.next_id(n).unwrap())
            .collect();
        assert_eq!(ids, (1..=SendHandles::MAX as u64).collect::<Vec<_>>());
        assert_eq!(
            s.next_id(SendHandles::MAX).unwrap_err().code_str(),
            "app.too_many_handles"
        );
        let t = SendHandles::default();
        assert_eq!(t.draft(1).unwrap_err().code_str(), "app.unknown_handle");
        assert_eq!(t.prepared(1).unwrap_err().code_str(), "app.unknown_handle");
        assert!(!t.close_draft(1));
        let epoch = t.epoch();
        t.clear();
        assert_eq!(t.epoch(), epoch + 1);
    }
}
