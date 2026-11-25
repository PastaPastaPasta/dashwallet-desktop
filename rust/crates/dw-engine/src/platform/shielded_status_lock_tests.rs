//! A shielded pass holds the coordinator's store to write for its whole
//! network stream (rs-platform-wallet `sync.rs`), a first scan for long.
//! Meanwhile the sync read (`shielded_sync_info`) and a bound key's receive
//! address answer from the engine's own state and the wallet's keys. A held
//! write guard stands for the pass.

use platform_wallet::PlatformEventHandler;
use platform_wallet::manager::shielded_sync::{ShieldedSyncPassSummary, WalletShieldedOutcome};
use tokio::sync::OwnedRwLockWriteGuard;

use super::super::shielded::{ShieldedAccount, ShieldedScanProgress, ShieldedSyncInfo};
use super::*;

/// How long a read may take while the store is held: well short of a scan.
const BOUND: Duration = Duration::from_secs(10);

type StoreGuard = OwnedRwLockWriteGuard<platform_wallet::wallet::shielded::FileBackedShieldedStore>;

/// The store's write guard, as a pass holds it from its watermark read on.
fn hold_store(engine: &Engine, session: &Arc<NetworkSession>) -> StoreGuard {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        Arc::clone(coordinator.store()).write_owned().await
    })
}

fn info(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> ShieldedSyncInfo {
    let dp = session.dashpay(wallet);
    engine
        .block_on(async { tokio::time::timeout(BOUND, dp.shielded_sync_info()).await })
        .expect("the sync read waited for the store a pass holds")
        .unwrap()
}

fn address(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> String {
    let dp = session.dashpay(wallet);
    engine
        .block_on(async { tokio::time::timeout(BOUND, dp.shielded_receive_address()).await })
        .expect("the receive address waited for the store a pass holds")
        .unwrap()
        .address
}

/// The library's own default address of `wallet`'s bound viewing key.
fn library_address(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> String {
    let m = session.manager().unwrap();
    engine.block_on(async move {
        let pw = m.get_wallet(&wallet.0).await.unwrap();
        let raw = pw
            .shielded_default_address(dw_vault::SHIELDED_ACCOUNT)
            .await
            .unwrap();
        dpp::address_funds::OrchardAddress::from_raw_bytes(&raw)
            .unwrap()
            .to_bech32m_string(dashcore::Network::Regtest)
    })
}

/// A first scan failing mid-stream: its tree appends made, no watermark
/// written, as the library's `?` on a stream error leaves it, and the
/// pass's outcome reported once it lets the store go. While it holds the
/// store, the sync read says bound and not established and Receive shows
/// the bound key's address; the full status waits. While the next pass
/// holds it, the sync read shows the failure. Once free, the full status
/// is not ready, with no balance.
#[test]
fn a_first_scan_failing_mid_stream_holds_neither_read_and_establishes_nothing() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    let expected = library_address(&engine, &session, wallet);
    let dp = session.dashpay(wallet);

    let mut guard = hold_store(&engine, &session);
    session.hub.on_shielded_tree_progress(2, 10);
    let scan = Some(ShieldedScanProgress {
        checked: 2,
        total: 10,
    });
    assert_eq!(session.hub.shielded_scan(), scan);
    let held = info(&engine, &session, wallet);
    assert!(held.active);
    assert_eq!(held.account, Some(ShieldedAccount::Bound));
    assert!(!held.established, "nothing is established mid-scan");
    assert!(!held.last_pass_failed);
    assert_eq!(address(&engine, &session, wallet), expected);
    let full = engine.block_on(async {
        tokio::time::timeout(Duration::from_millis(300), dp.shielded_status()).await
    });
    assert!(full.is_err(), "the full status reads the store");
    for n in 0..2u8 {
        guard.append_commitment(&[n; 32], false).unwrap();
    }
    drop(guard);
    session
        .hub
        .on_shielded_sync_completed(&ShieldedSyncPassSummary {
            wallet_results: [(wallet.0, WalletShieldedOutcome::Err("stream reset".into()))]
                .into_iter()
                .collect(),
            sync_unix_seconds: 1_700_000_000,
        });
    assert_eq!(session.hub.shielded_scan(), None);

    let next = hold_store(&engine, &session);
    let failed = info(&engine, &session, wallet);
    assert!(failed.last_pass_failed);
    assert!(!failed.established);
    assert_eq!(address(&engine, &session, wallet), expected);
    drop(next);

    let after = engine.block_on(dp.shielded_status()).unwrap();
    assert!(!after.ready, "a failed first scan shows no balance");
    assert_eq!((after.balance, after.synced_index), (0, None));
    assert!(after.last_pass_failed);
    assert!(!info(&engine, &session, wallet).established);
    assert_eq!(address(&engine, &session, wallet), expected);
}

/// Two wallets on the network's one coordinator, one established and the
/// other's last pass failed, the next pass holding the store: each
/// wallet's sync read and address are its own, whichever is read first.
#[test]
fn a_wallet_switch_during_a_scan_shows_each_wallets_own() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, a) = first_launch(&engine);
    let b = engine
        .block_on(session.import_wallet(
            Zeroizing::new(OTHER_PHRASE.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                ..Default::default()
            },
        ))
        .unwrap();
    assert!(bind(&engine, &session, b));
    let (a_address, b_address) = (
        library_address(&engine, &session, a),
        library_address(&engine, &session, b),
    );
    assert_ne!(a_address, b_address);
    {
        let mut guard = hold_store(&engine, &session);
        guard.set_last_synced_note_index(subwallet(a), 0).unwrap();
    }
    let a_status = engine.block_on(session.dashpay(a).shielded_status());
    assert!(a_status.unwrap().ready);
    session
        .hub
        .on_shielded_sync_completed(&ShieldedSyncPassSummary {
            wallet_results: [(b.0, WalletShieldedOutcome::Err("stream reset".into()))]
                .into_iter()
                .collect(),
            sync_unix_seconds: 1_700_000_000,
        });

    let _guard = hold_store(&engine, &session);
    for order in [[a, b], [b, a]] {
        for wallet in order {
            let held = info(&engine, &session, wallet);
            assert_eq!(held.account, Some(ShieldedAccount::Bound));
            assert_eq!(held.established, wallet == a, "{wallet:?}");
            assert_eq!(held.last_pass_failed, wallet == b, "{wallet:?}");
            let want = if wallet == a { &a_address } else { &b_address };
            assert_eq!(&address(&engine, &session, wallet), want);
        }
    }
}
