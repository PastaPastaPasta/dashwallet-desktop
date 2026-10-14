//! The self-test's commands (`--selftest`, G-02's U5/U6 measurements and
//! the gate screens), compiled and registered only with the `selftest`
//! feature (E0-13 D17). Release and packaged builds have neither these
//! commands nor their grant; `tauri-selftest.yml` builds a binary with
//! the feature. Their specs stay in every build, so the generated TypeScript
//! does not depend on the feature. Only `history_page_timed` and
//! `selftest_unlock` reach a facade.

// Without the feature only the specs remain, and the bodies' imports go unused.
#![cfg_attr(not(feature = "selftest"), allow(unused_imports))]

use std::io::Read;
use std::path::PathBuf;
use std::time::Instant;

use dw_vault::UnlockScope;
use zeroize::Zeroizing;

use crate::dto::LockStatus;
use crate::dto::history::{
    HistoryRequest, HistoryRows, TimedHistoryRows, TxCategoryName, TxRow, TxStatusInfo,
    TxStatusKindName, TxTypeName,
};
use crate::error::{AppCode, AppError};
use crate::ipc::commands;
use crate::state::parse_wallet_id;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

/// The grant of these commands, which `run` adds at start-up.
#[cfg(feature = "selftest")]
pub(crate) const CAPABILITY: &str = include_str!("../../capabilities-selftest/selftest.json");

commands! {
    #![cfg(feature = "selftest")]

    /// `--selftest`: writes the renderer's report, and exits once the report
    /// is final (`"final": false` is a checkpoint before a probe that could
    /// end the page).
    #[facade()]
    fn selftest_report(state, report: serde_json::Value) -> () {
        let path = state.selftest_path()?;
        let text = serde_json::to_string_pretty(&report)
            .map_err(|e| AppError::app(AppCode::Selftest, e.to_string()))?;
        tokio::fs::write(path, text)
            .await
            .map_err(|e| AppError::app(AppCode::Selftest, e.to_string()))?;
        eprintln!("dw-app: selftest report written to {}", path.display());
        if report.get("final").and_then(serde_json::Value::as_bool) != Some(false) {
            state.exit(0);
        }
        Ok(())
    }

    /// `--selftest`: the renderer shows `screen`; announced on stderr so a
    /// harness can capture the window during the pause that follows.
    #[facade()]
    fn selftest_checkpoint(state, screen: String) -> () {
        state.selftest_path()?;
        eprintln!("dw-app: selftest-screen {}", screen.escape_debug());
        Ok(())
    }

    /// `--selftest` with `--selftest-passphrase-file`: a full unlock with
    /// the file's first line, as `unlock` does, so the self-test records the
    /// screens of an encrypted vault without the renderer holding its
    /// passphrase. A file others could read or replace is refused
    /// (`app.selftest_passphrase_file`; Unix only, E0-13 D16).
    #[facade("NetworkSession::vault_op", "Vault::unlock")]
    fn selftest_unlock(state) -> LockStatus {
        state.selftest_path()?;
        let file = state.launch.selftest_passphrase_file.as_deref().ok_or_else(|| {
            AppError::app(
                AppCode::Selftest,
                "the app was not launched with --selftest-passphrase-file",
            )
        })?;
        let text = read_passphrase_file(file.to_path_buf()).await?;
        let passphrase = Zeroizing::new(text.lines().next().unwrap_or_default().as_bytes().to_vec());
        let session = state.session().await?;
        let status = session
            .vault_op(move |v| v.unlock(&passphrase, UnlockScope::Full))
            .await?;
        Ok(status.into())
    }

    /// A synthetic page of `rows` records, the size and shape of a real
    /// `historyPage` result: the IPC and serialisation cost alone (U5).
    /// Self-test runs only.
    #[facade()]
    fn ipc_bench_rows(state, rows: u32) -> HistoryRows {
        state.selftest_path()?;
        Ok(bench_rows(rows))
    }

    /// `historyPage` with the engine's share of the time (U5): the renderer
    /// subtracts `engineMicros` from its round trip. Self-test runs only.
    #[facade("NetworkSession::history_page")]
    fn history_page_timed(state, wallet_id: String, request: HistoryRequest) -> TimedHistoryRows {
        state.selftest_path()?;
        let id = parse_wallet_id(&wallet_id)?;
        let session = state.session().await?;
        let started = Instant::now();
        let page = session.history_page(id, request.into_query()).await?;
        let engine_micros = started.elapsed().as_micros() as u64;
        Ok(TimedHistoryRows {
            engine_micros,
            page: page.into(),
        })
    }
}

#[cfg(feature = "selftest")]
/// The passphrase file's text, if the file is the user's alone
/// (`dw_fs::open_owner_only`).
async fn read_passphrase_file(path: PathBuf) -> Result<Zeroizing<String>, AppError> {
    tokio::task::spawn_blocking(move || {
        let read = || {
            let mut file = dw_fs::open_owner_only(&path)?;
            // Sized up front: growing the buffer would leave a copy unzeroed.
            let len = file.metadata()?.len() as usize;
            let mut text = Zeroizing::new(String::with_capacity(len));
            file.read_to_string(&mut text)?;
            Ok::<_, std::io::Error>(text)
        };
        read().map_err(|e| {
            let message = format!("{}: {e}", path.display());
            AppError::app(AppCode::SelftestPassphraseFile, message)
        })
    })
    .await
    .map_err(|e| AppError::app(AppCode::Selftest, e.to_string()))?
}

#[cfg(feature = "selftest")]
fn bench_rows(rows: u32) -> HistoryRows {
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
                    involves_watch_only: false,
                    counterparty: None,
                }
            })
            .collect(),
        next_cursor: Some("bench".into()),
        total_matching: Some(10_000),
    }
}

#[cfg(all(test, feature = "selftest"))]
mod tests {
    use super::*;

    /// `Manager::add_capability` panics on a capability it cannot parse.
    #[test]
    fn the_capability_parses() {
        use tauri::ipc::RuntimeCapability;
        use tauri::utils::acl::capability::CapabilityFile;
        match CAPABILITY.build() {
            CapabilityFile::Capability(c) => {
                assert_eq!(c.identifier, "selftest");
                assert_eq!(c.permissions.len(), NAMES.len());
            }
            _ => panic!("one capability"),
        }
    }

    #[test]
    fn bench_page_is_a_full_page_of_distinct_records() {
        // A largest page must not overflow (debug builds check arithmetic).
        let page = bench_rows(10_000);
        assert_eq!(page.rows.len(), dw_engine::history::MAX_PAGE as usize);
        let txids: std::collections::HashSet<_> = page.rows.iter().map(|r| &r.txid).collect();
        assert_eq!(txids.len(), page.rows.len());
        assert!(page.rows.iter().all(|r| r.txid.len() == 64));
    }
}
