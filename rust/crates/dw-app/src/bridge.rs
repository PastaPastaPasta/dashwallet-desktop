//! The event bridge: every engine event reaches the renderer as one Tauri
//! event, `engine://event`, with a typed payload ([`AppEvent`]).
//!
//! Semantics (E0-13 D-row "events"):
//!
//! - **The engine never waits.** [`TauriSink::emit`] maps the event to its
//!   DTO (bounded work: at most [`MAX_TXIDS`] txids), offers it to a bounded
//!   queue ([`QUEUE`] events) and returns; it does no serialising or IPC. A
//!   forwarder thread emits each queued event.
//! - **Order.** One queue, one consumer, one Tauri event name: the renderer
//!   sees events in the order the engine sent them, across all kinds.
//! - **Bounded memory; an overflow is one signal.** When the webview falls
//!   [`QUEUE`] events behind, further events are dropped and the forwarder
//!   then sends `eventsOverflowed`, coalesced (one for a run of drops it has
//!   not reported yet): the renderer re-reads everything. Queued events are
//!   DTOs with bounded payloads, so the queue's memory is bounded too.
//!   The rate is bounded upstream (the engine debounces `sync`, `balances`,
//!   `historyChanged` and `platform` to 4 Hz per domain and `coinJoin` to
//!   1 Hz), so the queue fills only when the webview stalls.
//! - **One-shot news stays readable.** Every event's news is also state a
//!   command reads (D3 lists them); notices, which have no other home, are
//!   kept in a [`NoticeLog`] before the queue (`recentNotices`).
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

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

/// Events waiting for the webview at most; at ≤ [`MAX_TXIDS`] txids each, a
/// few MB.
pub const QUEUE: usize = 1024;

/// Notices [`NoticeLog`] keeps.
pub const NOTICES: usize = 64;

/// How long the forwarder waits for an event before it looks at the
/// overflow flag anyway: a drop recorded after the forwarder emptied the
/// queue is reported within this, even if the engine then goes quiet.
const OVERFLOW_CHECK: Duration = Duration::from_millis(250);

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
    /// Also kept for `recentNotices`, with its `seq`.
    Notice {
        network: Option<String>,
        code: NoticeCodeName,
        detail: String,
    },
    /// Not an engine event: the webview fell [`QUEUE`] events behind and
    /// later events were dropped. Re-read every query; read
    /// `recentNotices` for notices. Coalesced: one for the drops since the
    /// last one, sent after the event the forwarder was sending when they
    /// happened (or within 250 ms when it was idle); events queued before
    /// the drops may still follow it.
    EventsOverflowed,
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

/// A notice as `recentNotices` returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RecentNotice {
    /// Increases by one per notice, from 1, for the life of the process.
    pub seq: u64,
    pub network: Option<String>,
    pub code: NoticeCodeName,
    pub detail: String,
}

/// The last [`NOTICES`] notices, recorded before the event queue so an
/// overflow cannot lose them.
#[derive(Default)]
pub struct NoticeLog(Mutex<(u64, VecDeque<RecentNotice>)>);

impl NoticeLog {
    fn push(&self, network: Option<&DashNetwork>, code: NoticeCode, detail: &str) {
        let mut log = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (seq, notices) = &mut *log;
        *seq += 1;
        if notices.len() == NOTICES {
            notices.pop_front();
        }
        notices.push_back(RecentNotice {
            seq: *seq,
            network: network.map(DashNetwork::dir_name),
            code: code.into(),
            detail: detail.to_owned(),
        });
    }

    /// Oldest first.
    pub fn recent(&self) -> Vec<RecentNotice> {
        let log = self.0.lock().unwrap_or_else(|e| e.into_inner());
        log.1.iter().cloned().collect()
    }
}

/// The engine's sink: hands each event to the forwarder thread.
pub struct TauriSink {
    tx: mpsc::SyncSender<AppEvent>,
    /// Set when an event was dropped; the forwarder clears it and sends
    /// `eventsOverflowed`.
    overflowed: Arc<AtomicBool>,
    notices: Arc<NoticeLog>,
}

impl TauriSink {
    /// Starts the forwarder thread. It runs for the life of the process:
    /// it holds an `AppHandle`, which owns the engine and so this sink, so
    /// the queue never closes before exit; events still queued at exit
    /// are not sent (the window is gone).
    pub fn new<R: Runtime>(app: AppHandle<R>, notices: Arc<NoticeLog>) -> Result<Self, AppError> {
        Self::with_forwarder(QUEUE, notices, move |event| {
            if let Err(e) = app.emit(EVENT, event) {
                eprintln!("dw-app: could not forward an engine event: {e}");
            }
        })
    }

    /// The sink over a queue of `capacity` and a forwarder calling `emit`.
    fn with_forwarder(
        capacity: usize,
        notices: Arc<NoticeLog>,
        mut emit: impl FnMut(AppEvent) + Send + 'static,
    ) -> Result<Self, AppError> {
        let (tx, rx) = mpsc::sync_channel::<AppEvent>(capacity);
        let overflowed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&overflowed);
        std::thread::Builder::new()
            .name("dw-app-events".into())
            .spawn(move || {
                loop {
                    match rx.recv_timeout(OVERFLOW_CHECK) {
                        Ok(event) => emit(event),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    // A drop can be recorded after the queue drained (the
                    // engine thread paused between `Full` and the store):
                    // the timeout still brings the loop back here.
                    if flag.swap(false, Ordering::AcqRel) {
                        emit(AppEvent::EventsOverflowed);
                    }
                }
            })
            .map_err(|e| AppError::new(EngineCode::Io, format!("event thread: {e}")))?;
        Ok(TauriSink {
            tx,
            overflowed,
            notices,
        })
    }
}

impl EventSink for TauriSink {
    fn emit(&self, event: EngineEvent) {
        if let EngineEvent::Notice {
            network,
            code,
            detail,
        } = &event
        {
            self.notices.push(network.as_ref(), *code, detail);
        }
        match self.tx.try_send(AppEvent::from(event)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => self.overflowed.store(true, Ordering::Release),
            // Only once the forwarder is gone, while the app exits.
            Err(TrySendError::Disconnected(_)) => {}
        }
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
        let sink = TauriSink::new(app.handle().clone(), Arc::default()).unwrap();
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

    /// Sol r1 F3: a stalled webview fills the bounded queue; the engine
    /// keeps going, later events are dropped, and the renderer gets exactly
    /// one `eventsOverflowed`. Notices survive in the log; queued events are
    /// DTOs, their txids already cut.
    #[test]
    fn a_stalled_forwarder_overflows_once_and_stays_bounded() {
        const CAP: usize = 8;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let (stalled_tx, stalled) = std::sync::mpsc::channel::<()>();
        let record = Arc::clone(&seen);
        let notices = Arc::new(NoticeLog::default());
        let sink = TauriSink::with_forwarder(CAP, Arc::clone(&notices), move |e| {
            // The first event stalls the forwarder until the test lets go.
            if record.lock().unwrap().is_empty() {
                stalled_tx.send(()).unwrap();
                gate.recv().unwrap();
            }
            record.lock().unwrap().push(e);
        })
        .unwrap();
        let spv = |running| EngineEvent::SpvStateChanged {
            network: DashNetwork::Regtest,
            running,
        };
        let started = Instant::now();
        sink.emit(spv(true));
        stalled.recv().unwrap();
        while sink.tx.try_send(AppEvent::from(spv(true))).is_ok() {}
        // The queue is full: CAP events wait, the first is in flight. Every
        // further emit returns at once and is dropped, notices included.
        for i in 0..10_000 {
            sink.emit(EngineEvent::Notice {
                network: None,
                code: NoticeCode::BackupFailed,
                detail: format!("n{i}"),
            });
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the engine waited"
        );
        release.send(()).unwrap();
        // Bounded: the in-flight event, the one signal, then the CAP queued
        // events; none of the 10 000 notices got through.
        let deadline = Instant::now() + Duration::from_secs(20);
        while seen.lock().unwrap().len() < CAP + 2 {
            assert!(Instant::now() < deadline, "the forwarder did not drain");
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(100));
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), CAP + 2, "{seen:?}");
        assert_eq!(seen[1], AppEvent::EventsOverflowed);
        let spv_events = seen.iter().filter(|e| **e == AppEvent::from(spv(true)));
        assert_eq!(spv_events.count(), CAP + 1, "{seen:?}");
        // The notices are readable through the log, the last NOTICES of them.
        let recent = notices.recent();
        assert_eq!(recent.len(), NOTICES);
        assert_eq!(recent.last().unwrap().detail, "n9999");
        assert_eq!(recent.last().unwrap().seq, 10_000);
    }

    /// A drop recorded after the forwarder emptied the queue (the engine
    /// thread paused between `Full` and the flag) is still reported, though
    /// no event follows it.
    #[test]
    fn a_late_overflow_is_reported_by_an_idle_forwarder() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        let sink = TauriSink::with_forwarder(8, Arc::default(), move |e| {
            record.lock().unwrap().push(e);
        })
        .unwrap();
        sink.overflowed.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(20);
        while seen.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "the overflow was not reported");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(*seen.lock().unwrap(), [AppEvent::EventsOverflowed]);
    }
}
