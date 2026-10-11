//! The event bridge: every engine event reaches the renderer as one Tauri
//! event, `engine://event`, with a typed payload ([`AppEvent`]).
//!
//! Semantics (E0-13 D-row "events"):
//!
//! - **The engine never waits.** [`TauriSink::emit`] moves the event into a
//!   channel and returns; it does no mapping, serialising or IPC. A
//!   forwarder thread maps each event to its DTO and emits it.
//! - **Order.** One channel, one consumer, one Tauri event name: the renderer
//!   sees events in the order the engine sent them, across all kinds.
//! - **No drops, bounded rate.** The channel is unbounded and never drops,
//!   because some events happen once (`lockProgress` `done`,
//!   `dispatchResolved`). The rate is bounded upstream: the engine debounces
//!   `sync`, `balances`, `historyChanged` and `platform` to 4 Hz per domain
//!   and `coinJoin` to 1 Hz, keeping the last change of a burst.
//! - **Bounded payloads.** `historyChanged` sends at most [`MAX_TXIDS`]
//!   txids; a larger batch is sent empty, which the engine already defines
//!   as "reload the whole history". `newTransactions` sends the first
//!   [`MAX_TXIDS`] with the full `count`.
//! - **Hints, not rows.** Payloads say what changed; the renderer re-queries
//!   (DESIGN-opus §1.5 rule 4). Lease, lock and dispatch events carry their
//!   data because that data is the news.
//! - **Engine-side variants are forwarded.** dw-ffi withholds `platform`,
//!   `leaseChanged`, `lockProgress`, `dispatchResolved` and the E0-04/E0-05
//!   notices because the Swift shells are frozen; E0-04 §13, §16.7 and §16.8
//!   leave them engine-side "until E0-13", which is this binding.
//! - Events sent before the page listens are lost; the renderer queries
//!   everything when it starts.

use std::sync::mpsc;

use dw_engine::{DashNetwork, EngineEvent, EventSink, NoticeCode, PlatformChange};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use ts_rs::TS;

use crate::dto::platform::{DispatchSettled, Lease, LockProgress};
use crate::dto::{Balances, LockStateName, SyncStatus, mirror_enum};
use crate::error::{AppError, EngineCode};

/// The Tauri event every engine event is sent on.
pub const EVENT: &str = "engine://event";

/// Txids per `historyChanged` or `newTransactions` payload.
pub const MAX_TXIDS: usize = 256;

/// An engine event as the renderer receives it. `network` is the network's
/// directory name (`mainnet`, `testnet`, `regtest`, `devnet-<name>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AppEvent {
    SessionOpened {
        network: String,
    },
    SessionClosed {
        network: String,
    },
    /// A wallet was registered, or keys were attached to one: reload the list.
    WalletCreated {
        network: String,
        wallet_id: String,
    },
    WalletRemoved {
        network: String,
        wallet_id: String,
    },
    SpvStateChanged {
        network: String,
        running: bool,
    },
    /// ≤ 4 Hz, trailing edge kept.
    Sync {
        network: String,
        status: SyncStatus,
    },
    /// `null` while the scan has not reached the wallet's birth height.
    Balances {
        network: String,
        wallet_id: String,
        balances: Option<Balances>,
    },
    /// Transactions added or changed status. Empty `txids`: reload the
    /// whole history.
    HistoryChanged {
        network: String,
        wallet_id: String,
        txids: Vec<String>,
    },
    Notice {
        network: Option<String>,
        code: NoticeCodeName,
        detail: String,
    },
    VaultLockState {
        network: String,
        state: LockStateName,
    },
    /// Transactions seen for the first time, batched over 100 ms (QT-031…033).
    /// `catchUp`: SPV was not caught up (dash-qt shows no popups then).
    NewTransactions {
        network: String,
        wallet_id: String,
        /// The first [`MAX_TXIDS`] of the batch.
        txids: Vec<String>,
        /// The batch's size.
        count: u32,
        catch_up: bool,
    },
    WalletLoadChanged {
        network: String,
        wallet_id: String,
        loaded: bool,
    },
    /// Mixing status, CoinJoin balances or progress changed (≤ 1 Hz).
    CoinJoin {
        network: String,
        wallet_id: String,
    },
    /// DashPay data changed (DASHPAY §3.5, ≤ 4 Hz): re-query `change`'s domain.
    Platform {
        network: String,
        wallet_id: String,
        change: PlatformChangeName,
    },
    LeaseChanged {
        network: String,
        lease: Lease,
    },
    LockProgress {
        network: String,
        progress: LockProgress,
    },
    DispatchResolved {
        network: String,
        settled: DispatchSettled,
    },
}

/// The domain of a `platform` event. Identities are Base58.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "domain",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PlatformChangeName {
    /// Identities, their keys, profiles or names (contact views too).
    Identities,
    /// The identity's contacts, requests, contact accounts or ignored senders.
    Contacts {
        identity: String,
    },
    /// A tracked asset lock; `draft` is `null` for one no registration names (a top-up).
    Registration {
        draft: Option<String>,
    },
    Payments {
        identity: String,
    },
    /// The DPNS name states.
    Names,
}

impl From<PlatformChange> for PlatformChangeName {
    fn from(c: PlatformChange) -> Self {
        match c {
            PlatformChange::Identities => Self::Identities,
            PlatformChange::Contacts { identity } => Self::Contacts { identity },
            PlatformChange::Registration { draft } => Self::Registration { draft },
            PlatformChange::Payments { identity } => Self::Payments { identity },
            PlatformChange::Names => Self::Names,
        }
    }
}

/// Non-fatal conditions the UI may surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NoticeCodeName {
    /// The trusted quorum service is unreachable; proof checks retry lazily.
    PlatformContextUnavailable,
    /// dash-spv reported a fatal sync error.
    SpvError,
    /// Shutdown left a worker running (see the detail).
    UncleanShutdown,
    /// No SPV progress for 45 s while not caught up; offer `rotatePeers`.
    SyncStalled,
    /// An automatic wallet backup failed; the detail names the wallet and cause.
    BackupFailed,
    /// No longer sent (E0-04 DEC-134); kept as the engine keeps it.
    WalletSecretNotDeleted,
    /// A wallet's DashPay bring-up ended short of settled; read `dashpayStartup`.
    DashPayStartupIncomplete,
    /// A tracked asset lock has no dispatch record; it is kept and not sent.
    DispatchRecordMissing,
    /// A hand-off ran outside any dispatch scope (an engine bug); it was not sent.
    UnscopedDispatch,
    /// The dispatch journal cannot be opened or is newer than this build;
    /// leased hand-offs are refused.
    DispatchJournalUnavailable,
    /// SPV and the trusted quorum service disagree on a quorum key: every
    /// proof citing it is refused with `platform.trust_mismatch`. Once per
    /// quorum and session; the detail names the quorum.
    PlatformTrustMismatch,
}

mirror_enum!(NoticeCode => NoticeCodeName {
    PlatformContextUnavailable, SpvError, UncleanShutdown, SyncStalled, BackupFailed,
    WalletSecretNotDeleted, DashPayStartupIncomplete, DispatchRecordMissing, UnscopedDispatch,
    DispatchJournalUnavailable, PlatformTrustMismatch
});

impl From<EngineEvent> for AppEvent {
    fn from(event: EngineEvent) -> Self {
        let net = |n: DashNetwork| n.dir_name();
        let hex = |txids: Vec<dashcore::Txid>| -> Vec<String> {
            txids
                .iter()
                .take(MAX_TXIDS)
                .map(ToString::to_string)
                .collect()
        };
        match event {
            EngineEvent::SessionOpened { network } => AppEvent::SessionOpened {
                network: net(network),
            },
            EngineEvent::SessionClosed { network } => AppEvent::SessionClosed {
                network: net(network),
            },
            EngineEvent::WalletCreated { network, wallet_id } => AppEvent::WalletCreated {
                network: net(network),
                wallet_id: wallet_id.to_string(),
            },
            EngineEvent::WalletRemoved { network, wallet_id } => AppEvent::WalletRemoved {
                network: net(network),
                wallet_id: wallet_id.to_string(),
            },
            EngineEvent::SpvStateChanged { network, running } => AppEvent::SpvStateChanged {
                network: net(network),
                running,
            },
            EngineEvent::Sync { network, snapshot } => AppEvent::Sync {
                network: net(network),
                status: snapshot.into(),
            },
            EngineEvent::Balances {
                network,
                wallet_id,
                balances,
            } => AppEvent::Balances {
                network: net(network),
                wallet_id: wallet_id.to_string(),
                balances: balances.map(Into::into),
            },
            EngineEvent::HistoryChanged {
                network,
                wallet_id,
                txids,
            } => AppEvent::HistoryChanged {
                network: net(network),
                wallet_id: wallet_id.to_string(),
                // Too many to name: the engine's "reload the whole history".
                txids: if txids.len() > MAX_TXIDS {
                    Vec::new()
                } else {
                    hex(txids)
                },
            },
            EngineEvent::Notice {
                network,
                code,
                detail,
            } => AppEvent::Notice {
                network: network.map(net),
                code: code.into(),
                detail,
            },
            EngineEvent::VaultLockState { network, state } => AppEvent::VaultLockState {
                network: net(network),
                state: state.into(),
            },
            EngineEvent::NewTransactions {
                network,
                wallet_id,
                txids,
                catch_up,
            } => AppEvent::NewTransactions {
                network: net(network),
                wallet_id: wallet_id.to_string(),
                count: u32::try_from(txids.len()).unwrap_or(u32::MAX),
                txids: hex(txids),
                catch_up,
            },
            EngineEvent::WalletLoadChanged {
                network,
                wallet_id,
                loaded,
            } => AppEvent::WalletLoadChanged {
                network: net(network),
                wallet_id: wallet_id.to_string(),
                loaded,
            },
            EngineEvent::CoinJoin { network, wallet_id } => AppEvent::CoinJoin {
                network: net(network),
                wallet_id: wallet_id.to_string(),
            },
            EngineEvent::Platform {
                network,
                wallet_id,
                change,
            } => AppEvent::Platform {
                network: net(network),
                wallet_id: wallet_id.to_string(),
                change: change.into(),
            },
            EngineEvent::LeaseChanged { network, lease } => AppEvent::LeaseChanged {
                network: net(network),
                lease: lease.into(),
            },
            EngineEvent::LockProgress { network, phase } => AppEvent::LockProgress {
                network: net(network),
                progress: phase.into(),
            },
            EngineEvent::DispatchResolved { network, resolved } => AppEvent::DispatchResolved {
                network: net(network),
                settled: resolved.into(),
            },
        }
    }
}

/// The engine's sink: hands each event to the forwarder thread.
pub struct TauriSink {
    tx: mpsc::Sender<EngineEvent>,
}

impl TauriSink {
    /// Starts the forwarder thread. It runs for the life of the process:
    /// it holds an `AppHandle`, which owns the engine and so this sink, so
    /// the channel never closes before exit; events still queued at exit
    /// are not sent (the window is gone).
    pub fn new<R: Runtime>(app: AppHandle<R>) -> Result<Self, AppError> {
        let (tx, rx) = mpsc::channel::<EngineEvent>();
        std::thread::Builder::new()
            .name("dw-app-events".into())
            .spawn(move || {
                for event in rx {
                    if let Err(e) = app.emit(EVENT, AppEvent::from(event)) {
                        eprintln!("dw-app: could not forward an engine event: {e}");
                    }
                }
            })
            .map_err(|e| AppError::new(EngineCode::Io, format!("event thread: {e}")))?;
        Ok(TauriSink { tx })
    }
}

impl EventSink for TauriSink {
    fn emit(&self, event: EngineEvent) {
        // Fails only once the forwarder is gone, while the app exits.
        let _ = self.tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use dashcore::hashes::Hash;
    use dw_engine::WalletId;
    use dw_engine::platform::{
        DispatchResolution, DispatchResolved, FlowKind, LeaseStateView, LeaseView, LockPhase,
        LockReport,
    };
    use tauri::Listener;

    use super::*;

    fn json(e: EngineEvent) -> serde_json::Value {
        serde_json::to_value(AppEvent::from(e)).unwrap()
    }

    #[test]
    fn payloads_are_typed_and_camel_case() {
        let wallet_id = WalletId([7; 32]);
        assert_eq!(
            json(EngineEvent::Platform {
                network: DashNetwork::Testnet,
                wallet_id,
                change: PlatformChange::Contacts {
                    identity: "Bob".into()
                },
            }),
            serde_json::json!({
                "kind": "platform",
                "network": "testnet",
                "walletId": wallet_id.to_string(),
                "change": { "domain": "contacts", "identity": "Bob" }
            })
        );
        assert_eq!(
            json(EngineEvent::Notice {
                network: None,
                code: NoticeCode::DispatchJournalUnavailable,
                detail: "newer schema".into(),
            })["code"],
            "dispatchJournalUnavailable"
        );
        let lease = json(EngineEvent::LeaseChanged {
            network: DashNetwork::Regtest,
            lease: LeaseView {
                id: "0badcafe".into(),
                wallet_id: wallet_id.to_string(),
                flow: FlowKind::AcceptAndPay,
                state: LeaseStateView::Revoked {
                    cause: dw_engine::platform::RevokeCause::Lock,
                },
                own_key: true,
                key_expires_in_secs: Some(30),
                funds_committed: false,
                budgets: vec![dw_engine::platform::BudgetView {
                    purpose: dw_engine::platform::BudgetPurpose::Credits,
                    ceiling: 10_000_000_000_000_000,
                    spent: 0,
                }],
                in_flight: 0,
                call_running: false,
            },
        });
        assert_eq!(lease["lease"]["flow"], "acceptAndPay");
        assert_eq!(
            lease["lease"]["state"],
            serde_json::json!({"kind": "revoked", "cause": "lock"})
        );
        assert_eq!(lease["lease"]["budgets"][0]["ceiling"], "10000000000000000");
        let done = json(EngineEvent::LockProgress {
            network: DashNetwork::Regtest,
            phase: LockPhase::Done(LockReport {
                status: dw_vault::VaultStatus {
                    state: dw_vault::LockState::Locked,
                    encrypted: true,
                    quick_unlock_enrolled: false,
                    failed_attempts: 0,
                    retry_after_secs: None,
                    wallets_with_secrets: vec![],
                },
                flows: vec![],
            }),
        });
        assert_eq!(done["progress"]["kind"], "done");
        assert_eq!(done["progress"]["report"]["status"]["state"], "locked");
        let settled = json(EngineEvent::DispatchResolved {
            network: DashNetwork::Mainnet,
            resolved: DispatchResolved {
                wallet_id: wallet_id.to_string(),
                artifact: "registration/d1/funding".into(),
                resolution: DispatchResolution::NotSent,
            },
        });
        assert_eq!(settled["settled"]["resolution"], "notSent");
    }

    #[test]
    fn large_txid_batches_are_bounded() {
        let wallet_id = WalletId([1; 32]);
        let txids = |n: usize| -> Vec<dashcore::Txid> {
            (0..n)
                .map(|i| {
                    let mut b = [0u8; 32];
                    b[..8].copy_from_slice(&(i as u64).to_le_bytes());
                    dashcore::Txid::from_byte_array(b)
                })
                .collect()
        };
        let history = |n| {
            json(EngineEvent::HistoryChanged {
                network: DashNetwork::Regtest,
                wallet_id,
                txids: txids(n),
            })
        };
        assert_eq!(
            history(MAX_TXIDS)["txids"].as_array().unwrap().len(),
            MAX_TXIDS
        );
        assert_eq!(history(MAX_TXIDS + 1)["txids"], serde_json::json!([]));
        let new = json(EngineEvent::NewTransactions {
            network: DashNetwork::Regtest,
            wallet_id,
            txids: txids(1000),
            catch_up: true,
        });
        assert_eq!(new["count"], 1000);
        assert_eq!(new["txids"].as_array().unwrap().len(), MAX_TXIDS);
    }

    #[test]
    fn forwards_in_order_without_blocking_the_engine() {
        let app = tauri::test::mock_app();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        app.listen(EVENT, move |e| {
            let v: serde_json::Value = serde_json::from_str(e.payload()).unwrap();
            sink_seen.lock().unwrap().push(v["running"].clone());
        });
        let sink = TauriSink::new(app.handle().clone()).unwrap();
        for i in 0..200 {
            sink.emit(EngineEvent::SpvStateChanged {
                network: DashNetwork::Regtest,
                running: i % 2 == 0,
            });
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while seen.lock().unwrap().len() < 200 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 200);
        for (i, running) in seen.iter().enumerate() {
            assert_eq!(
                *running,
                serde_json::Value::Bool(i % 2 == 0),
                "event {i} out of order"
            );
        }
    }
}
