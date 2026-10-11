//! Tauri commands over dw-engine. Each is a thin 1:1 wrapper: argument
//! parsing, one engine call, a DTO conversion (DASHPAY R2).
//!
//! Every command the renderer may call is listed in `build.rs` and granted in
//! `capabilities/main.json`; anything else is refused by Tauri's ACL.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use dw_engine::{
    Engine, EngineConfig, HistoryFilter, HistoryQuery, NetworkSession, SessionOptions, WalletId,
};
use dw_vault::{UnlockScope, VaultConfig};
use tauri::{AppHandle, Runtime, State};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::bridge::TauriSink;
use crate::dto::{
    AppError, Balances, HistoryRequest, HistoryRows, LaunchInfo, LockStatus, SyncStatus,
    TimedHistoryRows, TxCategoryName, TxDetails, TxRow, TxStatusInfo, TxStatusKindName, TxTypeName,
    WalletSummary,
};
use crate::launch::LaunchOptions;

/// Process-wide app state, managed by Tauri.
pub struct AppState {
    pub launch: LaunchOptions,
    /// Taken at the top of `main`, for the cold-start measurement (U5).
    pub started: Instant,
    /// `None` in fixture mode: the renderer brings its own data.
    pub engine: Option<Engine>,
    session: Mutex<Option<Arc<NetworkSession>>>,
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
                Arc::new(TauriSink::new(app.clone())),
            )?)
        };
        Ok(AppState {
            launch,
            started,
            engine,
            session: Mutex::new(None),
        })
    }

    fn engine(&self) -> Result<&Engine, AppError> {
        self.engine
            .as_ref()
            .ok_or_else(|| AppError::app("fixture_mode", "the app runs on fixture data; no engine"))
    }

    /// The `--selftest` report path; the self-test commands refuse to run without it.
    fn selftest_path(&self) -> Result<&Path, AppError> {
        self.launch.selftest.as_deref().ok_or_else(|| {
            AppError::app("not_selftest", "the app was not launched with --selftest")
        })
    }

    async fn session(&self) -> Result<Arc<NetworkSession>, AppError> {
        self.session
            .lock()
            .await
            .clone()
            .ok_or_else(|| AppError::app("network_not_open", "call open_network first"))
    }
}

fn parse_wallet_id(s: &str) -> Result<WalletId, AppError> {
    s.parse().map_err(AppError::from)
}

// Commands that do no I/O are still `async`: Tauri runs sync commands on the
// main thread, and the measured IPC path (U5) must be the one the engine
// calls take.
#[tauri::command(async)]
pub fn launch_info(state: State<'_, AppState>) -> LaunchInfo {
    LaunchInfo::new(&state.launch)
}

/// Opens the launch network (once; later calls return the same session) and
/// starts SPV when the app was launched with `--sync`. A failed SPV start
/// leaves the session open (the wallet stays readable offline); the status
/// bar shows that SPV is not running.
#[tauri::command]
pub async fn open_network(state: State<'_, AppState>) -> Result<LockStatus, AppError> {
    let engine = state.engine()?;
    let mut slot = state.session.lock().await;
    let session = match &*slot {
        Some(s) => Arc::clone(s),
        None => {
            let opts = SessionOptions {
                dapi_addresses: state.launch.dapi_addresses.clone(),
                spv_peers: state.launch.spv_peers.clone(),
                no_platform: state.launch.no_platform,
                ..SessionOptions::default()
            };
            let s = engine
                .open_network(state.launch.network.clone(), opts)
                .await?;
            *slot = Some(Arc::clone(&s));
            if state.launch.sync
                && let Err(e) = s.start_spv().await
            {
                eprintln!("dw-app: SPV did not start: {e}");
            }
            s
        }
    };
    Ok(session.vault().status().into())
}

#[tauri::command]
pub async fn list_wallets(state: State<'_, AppState>) -> Result<Vec<WalletSummary>, AppError> {
    let infos = state.session().await?.wallet_infos()?;
    Ok(infos.into_iter().map(WalletSummary::from).collect())
}

#[tauri::command]
pub async fn lock_status(state: State<'_, AppState>) -> Result<LockStatus, AppError> {
    Ok(state.session().await?.vault().status().into())
}

/// Full unlock with the vault passphrase. The passphrase arrives as a JSON
/// string; the copy this process owns is zeroed on drop. The renderer's
/// copy (DOM value, JS string) cannot be zeroed: see the H-01 note in
/// `docs/research/g-01-tauri-spike.md`.
#[tauri::command]
pub async fn unlock(
    state: State<'_, AppState>,
    passphrase: String,
) -> Result<LockStatus, AppError> {
    let passphrase = Zeroizing::new(passphrase);
    let session = state.session().await?;
    let status = session
        .vault_op(move |v| v.unlock(passphrase.as_bytes(), UnlockScope::Full))
        .await?;
    Ok(status.into())
}

#[tauri::command]
pub async fn lock(state: State<'_, AppState>) -> Result<LockStatus, AppError> {
    // The full lock (E0-04 §8.1): revokes leases and returns after their drain.
    Ok(state.session().await?.lock_vault().await?.status.into())
}

#[tauri::command]
pub async fn balance(
    state: State<'_, AppState>,
    wallet_id: String,
) -> Result<Option<Balances>, AppError> {
    let id = parse_wallet_id(&wallet_id)?;
    Ok(state.session().await?.balances(&id)?.map(Balances::from))
}

#[tauri::command]
pub async fn sync_status(state: State<'_, AppState>) -> Result<SyncStatus, AppError> {
    Ok(state.session().await?.sync_snapshot()?.into())
}

#[tauri::command]
pub async fn history_page(
    state: State<'_, AppState>,
    wallet_id: String,
    request: HistoryRequest,
) -> Result<HistoryRows, AppError> {
    let id = parse_wallet_id(&wallet_id)?;
    let session = state.session().await?;
    Ok(session
        .history_page(id, history_query(request))
        .await?
        .into())
}

fn history_query(request: HistoryRequest) -> HistoryQuery {
    HistoryQuery {
        filter: HistoryFilter {
            categories: request.categories.into_iter().map(Into::into).collect(),
            text: request.text.filter(|t| !t.trim().is_empty()),
            ..HistoryFilter::default()
        },
        sort: request.sort.into(),
        cursor: request.cursor,
        limit: request.limit,
    }
}

/// `history_page` with the engine's share of the time (U5): the renderer
/// subtracts `engine_micros` from its round trip to get the IPC and
/// serialisation cost of a real page, apart from the engine's query cost.
#[tauri::command]
pub async fn history_page_timed(
    state: State<'_, AppState>,
    wallet_id: String,
    request: HistoryRequest,
) -> Result<TimedHistoryRows, AppError> {
    let id = parse_wallet_id(&wallet_id)?;
    let session = state.session().await?;
    let started = Instant::now();
    let page = session.history_page(id, history_query(request)).await?;
    let engine_micros = started.elapsed().as_micros() as u64;
    Ok(TimedHistoryRows {
        engine_micros,
        page: page.into(),
    })
}

#[tauri::command]
pub async fn tx_detail(
    state: State<'_, AppState>,
    wallet_id: String,
    txid: String,
) -> Result<TxDetails, AppError> {
    let id = parse_wallet_id(&wallet_id)?;
    Ok(state.session().await?.tx_detail(id, txid).await?.into())
}

/// A start-up milestone of the renderer (`shell`: the first frame of the
/// window; `first-paint`: the first Overview frame with the balance and
/// history). Returns milliseconds since the top of `main` and logs them
/// (U5 cold start).
#[tauri::command(async)]
pub fn startup_mark(state: State<'_, AppState>, mark: String) -> u64 {
    let ms = state.started.elapsed().as_millis() as u64;
    eprintln!("dw-app: startup {mark} {ms}");
    ms
}

/// `--selftest`: writes the renderer's report, and exits once the report is
/// marked final (`"final": false` is a checkpoint before a probe that could
/// end the page).
#[tauri::command(async)]
pub fn selftest_report<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    report: serde_json::Value,
) -> Result<(), AppError> {
    let path = state.selftest_path()?;
    let text = serde_json::to_string_pretty(&report)
        .map_err(|e| AppError::app("selftest", e.to_string()))?;
    std::fs::write(path, text).map_err(|e| AppError::app("selftest", e.to_string()))?;
    eprintln!("dw-app: selftest report written to {}", path.display());
    if report.get("final").and_then(serde_json::Value::as_bool) != Some(false) {
        app.exit(0);
    }
    Ok(())
}

/// `--selftest`: the renderer shows `screen`; announced on stderr so a
/// harness can capture the window during the pause that follows.
#[tauri::command(async)]
pub fn selftest_checkpoint(state: State<'_, AppState>, screen: String) -> Result<(), AppError> {
    state.selftest_path()?;
    eprintln!("dw-app: selftest-screen {screen}");
    Ok(())
}

/// Synthetic page of `rows` records, the size and shape of a real
/// `history_page` result. Measures the IPC and serialisation cost alone
/// (U5); the engine's own query cost is measured on the engine.
#[tauri::command(async)]
pub fn ipc_bench_rows(rows: u32) -> HistoryRows {
    let rows = rows.min(dw_engine::history::MAX_PAGE);
    HistoryRows {
        rows: (0..rows)
            .map(|i| {
                let sent = i % 3 == 0;
                TxRow {
                    txid: format!("{:064x}", u64::from(i).wrapping_mul(0x9e37_79b9_7f4a_7c15)),
                    record_index: 0,
                    tx_type: if sent {
                        TxTypeName::SendToAddress
                    } else {
                        TxTypeName::RecvWithAddress
                    },
                    category: if sent {
                        TxCategoryName::Sent
                    } else {
                        TxCategoryName::Received
                    },
                    status: TxStatusInfo {
                        kind: TxStatusKindName::Confirmed,
                        confirmations: 6 + i,
                        instant_locked: true,
                        chain_locked: true,
                        matures_in: None,
                    },
                    timestamp: Some(1_790_000_000 - u64::from(i) * 600),
                    block_height: Some(100_000 - i),
                    amount: if sent { -150_000_226 } else { 25_000_000 },
                    fee: sent.then_some(226),
                    address: Some("yNBLvUCmZq3K8v4eusVmKavEj2XrPoQe1x".into()),
                    label: (i % 5 == 0).then(|| "Coffee refund".into()),
                    counts_toward_balance: true,
                }
            })
            .collect(),
        next_cursor: Some("bench".into()),
        total_matching: Some(10_000),
    }
}

/// Registered but granted to no window: the U6 probe that the ACL refuses an
/// unlisted command. It must stay out of `capabilities/`.
#[tauri::command]
pub fn probe_unlisted() -> &'static str {
    "reached"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_page_is_a_full_page_of_distinct_records() {
        // A largest page must not overflow (debug builds check arithmetic).
        let page = ipc_bench_rows(10_000);
        assert_eq!(page.rows.len(), dw_engine::history::MAX_PAGE as usize);
        let txids: std::collections::HashSet<_> = page.rows.iter().map(|r| &r.txid).collect();
        assert_eq!(txids.len(), page.rows.len());
        assert!(page.rows.iter().all(|r| r.txid.len() == 64));
    }
}
