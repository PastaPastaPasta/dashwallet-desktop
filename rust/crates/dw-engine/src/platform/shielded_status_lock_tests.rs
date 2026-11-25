//! A shielded pass holds the coordinator's store to write for its whole
//! network stream (rs-platform-wallet `sync.rs`), a first scan for long.
//! Meanwhile the account's status and its receive address answer from the
//! engine's own state: the account and the bound viewing key's address,
//! and no ledger (not ready). With kept ledgers on
//! (`SHOW_KEPT_SHIELDED_LEDGER`), the ledger kept from the last read under
//! the store's guard: none while the account was never established, nor
//! after a change began since. A held write guard stands for the pass.

use platform_wallet::PlatformEventHandler;
use platform_wallet::manager::shielded_sync::{ShieldedSyncPassSummary, WalletShieldedOutcome};
use tokio::sync::OwnedRwLockWriteGuard;

use super::super::shielded::{ShieldedAccount, ShieldedStatus};
use super::*;

/// How long a read may take while the store is held: well past the
/// status's own wait, well short of a scan.
const BOUND: Duration = Duration::from_secs(3);

type StoreGuard = OwnedRwLockWriteGuard<platform_wallet::wallet::shielded::FileBackedShieldedStore>;

/// The store's write guard, as a pass holds it from its watermark read on.
fn hold_store(engine: &Engine, session: &Arc<NetworkSession>) -> StoreGuard {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        Arc::clone(coordinator.store()).write_owned().await
    })
}

fn status(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> ShieldedStatus {
    let dp = session.dashpay(wallet);
    engine
        .block_on(async { tokio::time::timeout(BOUND, dp.shielded_status()).await })
        .expect("the status waited for the store a pass holds")
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

/// Sets `wallet`'s watermark as a completed scan of this session does,
/// which establishes its ledger.
fn scanned(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId, watermark: u64) {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let mut store = coordinator.store().write().await;
        store
            .set_last_synced_note_index(subwallet(wallet), watermark)
            .unwrap();
    });
}

fn import_other(engine: &Engine, session: &Arc<NetworkSession>) -> WalletId {
    engine
        .block_on(session.import_wallet(
            Zeroizing::new(OTHER_PHRASE.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                ..Default::default()
            },
        ))
        .unwrap()
}

/// A first scan: the account is bound, nothing established. The status
/// says so at once and Receive shows the bound key's address, both while
/// the pass holds the store.
#[test]
fn a_first_scan_holds_neither_the_status_nor_the_address() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    let expected = library_address(&engine, &session, wallet);

    let guard = hold_store(&engine, &session);
    let held = status(&engine, &session, wallet);
    assert!(held.active);
    assert_eq!(held.account, Some(ShieldedAccount::Bound));
    assert!(
        !held.ready,
        "nothing is established before the first scan ends"
    );
    assert_eq!(
        (held.balance, held.pending, held.synced_index),
        (0, 0, None)
    );
    assert_eq!(address(&engine, &session, wallet), expected);
    drop(guard);

    let after = status(&engine, &session, wallet);
    assert_eq!(after.account, Some(ShieldedAccount::Bound));
    assert!(!after.ready);
}

/// A pass that fails mid-stream (its tree appends made, no watermark
/// written, as the library's `?` on a stream error leaves it) establishes
/// nothing: once it lets the store go, the status reads the store itself,
/// not ready, the failure shown; the address is unchanged.
#[test]
fn a_scan_failing_mid_stream_establishes_nothing() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    let expected = address(&engine, &session, wallet);

    let mut guard = hold_store(&engine, &session);
    session.hub.on_shielded_tree_progress(2, 10);
    for n in 0..2u8 {
        guard.append_commitment(&[n; 32], false).unwrap();
    }
    assert!(!status(&engine, &session, wallet).ready);
    assert_eq!(address(&engine, &session, wallet), expected);
    session
        .hub
        .on_shielded_sync_completed(&ShieldedSyncPassSummary {
            wallet_results: [(wallet.0, WalletShieldedOutcome::Err("stream reset".into()))]
                .into_iter()
                .collect(),
            sync_unix_seconds: 1_700_000_000,
        });
    drop(guard);

    let after = status(&engine, &session, wallet);
    assert!(!after.ready, "a partial scan shows no balance");
    assert_eq!((after.balance, after.synced_index), (0, None));
    assert!(after.last_pass_failed);
    assert_eq!(after.scan, None);
    assert_eq!(address(&engine, &session, wallet), expected);
}

/// A note of `value` credits at tree position `position`, as a scan saves
/// one.
fn note(position: u64, value: u64) -> platform_wallet::wallet::shielded::ShieldedNote {
    platform_wallet::wallet::shielded::ShieldedNote {
        position,
        cmx: [position as u8; 32],
        nullifier: [position as u8 + 100; 32],
        block_height: 1,
        is_spent: false,
        value,
        note_data: Vec::new(),
    }
}

/// Kept ledgers off (the default): an established account shows no
/// ledger while a pass holds the store, not ready rather than a figure
/// read before the pass, and its ledger again once the pass lets go.
#[test]
fn by_default_an_established_account_shows_no_ledger_during_a_pass() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    let mut guard = hold_store(&engine, &session);
    guard.save_note(subwallet(wallet), &note(0, 5_000)).unwrap();
    guard
        .set_last_synced_note_index(subwallet(wallet), 1)
        .unwrap();
    drop(guard);
    let before = status(&engine, &session, wallet);
    assert!(before.ready);
    assert_eq!(before.balance, 5_000);

    let guard = hold_store(&engine, &session);
    let held = status(&engine, &session, wallet);
    assert!(!held.ready, "no ledger while the pass holds the store");
    assert_eq!(
        (held.balance, held.synced_index, held.pending),
        (0, None, 0)
    );
    assert_eq!(held.account, Some(ShieldedAccount::Bound));
    drop(guard);

    let after = status(&engine, &session, wallet);
    assert!(after.ready);
    assert_eq!(after.balance, 5_000);
}

/// An established account keeps showing its ledger as last read under the
/// store's guard while a later pass holds it. A signing call (through the
/// engine's own entry for them) keeps no ledger read while it runs and
/// shows none kept before it: here its spend reserves the note, and a
/// stalled store write meanwhile shows not ready, never the balance from
/// before the reservation.
#[test]
fn a_kept_ledger_is_never_shown_during_or_after_a_change() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    session.hub.set_shielded_keep(true);
    let mut guard = hold_store(&engine, &session);
    guard.save_note(subwallet(wallet), &note(0, 5_000)).unwrap();
    guard
        .set_last_synced_note_index(subwallet(wallet), 1)
        .unwrap();
    drop(guard);
    let before = status(&engine, &session, wallet);
    assert!(before.ready, "established by the scan");
    assert_eq!((before.balance, before.synced_index), (5_000, Some(1)));

    let guard = hold_store(&engine, &session);
    let held = status(&engine, &session, wallet);
    assert!(held.ready, "the ledger read before the pass");
    assert_eq!((held.balance, held.synced_index), (5_000, Some(1)));
    drop(guard);

    let s = Arc::clone(&session);
    let (fresh, during, guard) = engine
        .block_on(pool::on_pool_signing(
            &session,
            wallet,
            move |_, _| async move {
                let fresh = s.dashpay(wallet).shielded_status().await?;
                let m = s.manager().unwrap();
                let coordinator = pool::coordinator(&s, &m, None).await?;
                let mut guard = Arc::clone(coordinator.store()).write_owned().await;
                assert!(guard.mark_pending(subwallet(wallet), &[100; 32]).unwrap());
                let during = tokio::time::timeout(BOUND, s.dashpay(wallet).shielded_status())
                    .await
                    .expect("the status waited for the store")?;
                Ok((fresh, during, guard))
            },
        ))
        .unwrap();
    assert!(fresh.ready, "a free store is read during a change");
    assert_eq!(fresh.balance, 5_000);
    assert!(!during.ready, "no kept ledger during a change");
    assert_eq!((during.balance, during.synced_index), (0, None));
    assert_eq!(during.account, Some(ShieldedAccount::Bound));
    let after = status(&engine, &session, wallet);
    assert!(!after.ready, "nothing read during the change is kept");
    assert_eq!(after.balance, 0);
    drop(guard);

    let read = status(&engine, &session, wallet);
    assert!(read.ready, "read again");
    assert!(read.balance < 5_000, "the reserved note is not spendable");
}

/// A status read that began before an unload keeps nothing after it: the
/// unload moves the generation, so the read's ledger is refused.
#[test]
fn a_read_racing_an_unload_keeps_nothing() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    session.hub.set_shielded_keep(true);
    let generation = session.hub.shielded_generation().unwrap();
    engine.block_on(session.unload_wallet(wallet)).unwrap();
    let ledger = pool::LedgerView {
        unresolved_spends: Vec::new(),
        ledger: Some((5_000, Some(1), 0)),
    };
    session.hub.keep_shielded_ledger(wallet, generation, ledger);
    assert_eq!(session.hub.shielded_ledger(wallet), None);
}

/// Two wallets on the network's one coordinator, the pass holding its
/// store: each wallet's status and address are its own, and one wallet's
/// kept ledger never answers for the other.
#[test]
fn a_wallet_switch_during_a_scan_shows_each_wallets_own() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, a) = first_launch(&engine);
    session.hub.set_shielded_keep(true);
    let b = import_other(&engine, &session);
    assert!(bind(&engine, &session, b));
    let (a_address, b_address) = (
        library_address(&engine, &session, a),
        library_address(&engine, &session, b),
    );
    assert_ne!(a_address, b_address);
    scanned(&engine, &session, a, 0);
    assert!(status(&engine, &session, a).ready);
    assert!(!status(&engine, &session, b).ready);

    let guard = hold_store(&engine, &session);
    for _ in 0..2 {
        let (sa, sb) = (status(&engine, &session, a), status(&engine, &session, b));
        assert!(sa.ready, "a's own kept ledger");
        assert!(!sb.ready, "never a's ledger for b");
        assert_eq!(sb.account, Some(ShieldedAccount::Bound));
        assert_eq!(address(&engine, &session, a), a_address);
        assert_eq!(address(&engine, &session, b), b_address);
    }
    drop(guard);
}
