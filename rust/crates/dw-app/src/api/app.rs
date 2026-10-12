//! The binding's own commands: launch facts, the start-up and self-test
//! hooks (G-02's U5/U6 measurements) and the refusal probe. None reaches a
//! facade.

use std::time::Instant;

use crate::bridge::RecentNotice;
use crate::dto::LaunchInfo;
use crate::dto::history::{
    HistoryRequest, HistoryRows, TimedHistoryRows, TxCategoryName, TxRow, TxStatusInfo,
    TxStatusKindName, TxTypeName,
};
use crate::error::{AppCode, AppError};
use crate::ipc::commands;
use crate::state::parse_wallet_id;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

commands! {
    /// How the app was launched: fixture or engine mode, the self-test, the
    /// forced theme and the network `openNetwork` opens by default.
    #[facade()]
    fn launch_info(state) -> LaunchInfo {
        Ok(LaunchInfo::new(&state.launch))
    }

    /// The last 64 engine notices, oldest first, kept apart from the event
    /// queue: after `eventsOverflowed`, the notices it may have dropped.
    #[facade()]
    fn recent_notices(state) -> Vec<RecentNotice> {
        Ok(state.memory.recent_notices())
    }

    /// A start-up milestone of the renderer (`shell`: the window's first
    /// frame; `first-paint`: the first Overview frame with data). Returns
    /// milliseconds since the top of `main` and logs them (U5 cold start).
    #[facade()]
    fn startup_mark(state, mark: String) -> u64 {
        let ms = state.started.elapsed().as_millis() as u64;
        // Escaped: the harness reads these lines, so a mark cannot forge one.
        eprintln!("dw-app: startup {} {ms}", mark.escape_debug());
        Ok(ms)
    }

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

    /// A synthetic page of `rows` records, the size and shape of a real
    /// `historyPage` result: the IPC and serialisation cost alone (U5).
    #[facade()]
    fn ipc_bench_rows(state, rows: u32) -> HistoryRows {
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

    /// Registered but granted by no capability: the U6 probe that the ACL
    /// refuses an unlisted command (`api::UNGRANTED`).
    #[facade()]
    fn probe_unlisted(state) -> String {
        Ok("reached".into())
    }
}

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
                }
            })
            .collect(),
        next_cursor: Some("bench".into()),
        total_matching: Some(10_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
