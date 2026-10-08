//! The event bridge: engine events become `engine://signal` events in the
//! renderer, which turns them into query invalidations
//! (`apps/desktop/src/api/events.ts`).

use dw_engine::{EngineEvent, EventSink};
use tauri::{AppHandle, Emitter, Runtime};

use crate::dto::EngineSignal;

/// Name of the Tauri event every engine signal is sent on.
pub const SIGNAL_EVENT: &str = "engine://signal";

/// Maps an engine event to the signal the renderer acts on. Events the gate
/// screens do not use (CoinJoin, wallet load) map to the nearest signal so a
/// later screen never misses a change.
pub fn signal_of(event: &EngineEvent) -> EngineSignal {
    let net = |n: &dw_engine::DashNetwork| n.dir_name();
    match event {
        EngineEvent::SessionOpened { network } => EngineSignal::Session {
            network: net(network),
            open: true,
        },
        EngineEvent::SessionClosed { network } => EngineSignal::Session {
            network: net(network),
            open: false,
        },
        EngineEvent::WalletCreated { network, .. }
        | EngineEvent::WalletRemoved { network, .. }
        | EngineEvent::WalletLoadChanged { network, .. } => EngineSignal::Wallets {
            network: net(network),
        },
        EngineEvent::SpvStateChanged { network, .. } | EngineEvent::Sync { network, .. } => {
            EngineSignal::Sync {
                network: net(network),
            }
        }
        EngineEvent::Balances {
            network, wallet_id, ..
        }
        | EngineEvent::CoinJoin { network, wallet_id } => EngineSignal::Balances {
            network: net(network),
            wallet_id: wallet_id.to_string(),
        },
        EngineEvent::HistoryChanged {
            network, wallet_id, ..
        }
        | EngineEvent::NewTransactions {
            network, wallet_id, ..
        } => EngineSignal::History {
            network: net(network),
            wallet_id: wallet_id.to_string(),
        },
        EngineEvent::VaultLockState { network, .. } => EngineSignal::LockState {
            network: net(network),
        },
        EngineEvent::Notice { code, detail, .. } => EngineSignal::Notice {
            code: format!("{code:?}"),
            detail: detail.clone(),
        },
    }
}

/// Engine sink that forwards every event to the app's windows.
pub struct TauriSink<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> TauriSink<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> EventSink for TauriSink<R> {
    fn emit(&self, event: EngineEvent) {
        // `emit` queues the message for the webview and does not block.
        if let Err(e) = self.app.emit(SIGNAL_EVENT, signal_of(&event)) {
            eprintln!("dw-app: could not forward an engine event: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dw_engine::{DashNetwork, WalletId};

    #[test]
    fn signals_name_the_network_and_wallet() {
        let wallet_id = WalletId([7; 32]);
        let s = signal_of(&EngineEvent::HistoryChanged {
            network: DashNetwork::Regtest,
            wallet_id,
            txids: vec![],
        });
        assert_eq!(
            s,
            EngineSignal::History {
                network: "regtest".into(),
                wallet_id: wallet_id.to_string()
            }
        );
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["kind"], "history");
        assert_eq!(json["walletId"], wallet_id.to_string());
    }

    #[test]
    fn lock_and_sync_events_map_to_their_signals() {
        let lock = signal_of(&EngineEvent::VaultLockState {
            network: DashNetwork::Testnet,
            state: dw_vault::LockState::Unlocked,
        });
        assert_eq!(
            lock,
            EngineSignal::LockState {
                network: "testnet".into()
            }
        );
        let spv = signal_of(&EngineEvent::SpvStateChanged {
            network: DashNetwork::Mainnet,
            running: true,
        });
        assert_eq!(
            spv,
            EngineSignal::Sync {
                network: "mainnet".into()
            }
        );
    }

    #[test]
    fn forwards_through_a_mock_app() {
        use std::sync::{Arc, Mutex};
        use tauri::Listener;
        let app = tauri::test::mock_app();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        app.listen(SIGNAL_EVENT, move |e| {
            sink_seen.lock().unwrap().push(e.payload().to_string())
        });
        TauriSink::new(app.handle().clone()).emit(EngineEvent::SessionOpened {
            network: DashNetwork::Regtest,
        });
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].contains(r#""kind":"session""#), "{}", seen[0]);
    }
}
