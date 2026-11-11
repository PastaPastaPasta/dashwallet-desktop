//! DEC-1046 (E1 review r1, R1-1): no close, unload or quit deletes a
//! shielded spend whose result is not known. At the pin, platform-wallet's
//! `remove_wallet` always purges the wallet's shielded store, its stored
//! transitions and reservations included (`wallet_lifecycle.rs:847-850` →
//! `coordinator.rs:878-902` → `file_store.rs:880-901`), and it has no detach.
//! So Close Wallet and an in-app network close or switch are refused while a
//! wallet holds one, and the quit keeps that wallet loaded: its rows are on
//! disk, and the next launch rehydrates them as pending.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, UnlockScope, VaultConfig};
use platform_wallet::wallet::shielded::store::{PendingRedrive, StalePendingSpend};
use platform_wallet::wallet::shielded::{ShieldedStore, SubwalletId};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::runtime::StoredSpends;
use super::shielded::UnresolvedSpend;
use super::shielded_pool::{self as pool, on_pool};
use super::topup_tests::{PASS, PHRASE, options};
use crate::{
    DashNetwork, Engine, EngineConfig, EngineError, ImportOptions, NetworkSession, WalletId,
};

/// A second wallet's recovery phrase (BIP-39's test vector).
const OTHER_PHRASE: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";

/// Where the quitting child process keeps its data root.
const QUIT_ROOT: &str = "DW_E1_QUIT_ROOT";

struct NullSink;
impl crate::EventSink for NullSink {
    fn emit(&self, _: crate::EngineEvent) {}
}

/// An engine over `root` whose passphrase vault needs no OS store, so a
/// second process opens it as the first left it.
fn engine_at(root: &Path) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.join("data"),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(NullSink),
    )
    .unwrap()
}

fn open(engine: &Engine) -> Arc<NetworkSession> {
    engine
        .block_on(engine.open_network(DashNetwork::Regtest, options()))
        .unwrap()
}

/// A new vault and the "abandon … about" wallet, its viewing key bound
/// (and so persisted in wallet.sqlite).
fn first_launch(engine: &Engine) -> (Arc<NetworkSession>, WalletId) {
    let (session, wallet) = unbound_launch(engine);
    assert!(bind(engine, &session, wallet));
    (session, wallet)
}

/// [`first_launch`] before the bind: no shielded call has run yet, so the
/// shielded store is not open (a bind opens it).
fn unbound_launch(engine: &Engine) -> (Arc<NetworkSession>, WalletId) {
    unbound_launch_with(engine, options())
}

fn unbound_launch_with(
    engine: &Engine,
    options: crate::SessionOptions,
) -> (Arc<NetworkSession>, WalletId) {
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, options))
        .unwrap();
    engine
        .block_on(session.vault_op(|v| v.create(Some(PASS))))
        .unwrap();
    let wallet = engine
        .block_on(session.import_wallet(
            Zeroizing::new(PHRASE.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                core_compat: false,
                ..Default::default()
            },
        ))
        .unwrap();
    let manager = session.manager().unwrap();
    assert!(
        engine.block_on(manager.shielded_coordinator()).is_none(),
        "the shielded store opens on first use"
    );
    (session, wallet)
}

fn bind(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> bool {
    engine
        .block_on(on_pool(session, wallet, |_, lib| async move {
            Ok(pool::ShieldedLib::bind(&lib).await?)
        }))
        .unwrap()
}

/// Two spends of unknown result as the library stores them (operations.rs
/// `arm_redrive_record`): a transfer's and an unshield's serialized
/// transitions (dpp `ShieldedTransfer` and `ShieldedWithdrawal`, the
/// library's `transfer` and `withdraw`), their nullifiers reserved against
/// the anchor they were built on.
fn spends() -> Vec<PendingRedrive> {
    (1u8..=2)
        .map(|n| {
            let (anchor, nullifiers) = ([0xa0 + n; 32], vec![[0x10 + n; 32], [0x20 + n; 32]]);
            PendingRedrive {
                activity_id: [n; 32],
                st_bytes: transition(n, anchor, &nullifiers),
                anchor,
                nullifiers,
                attempts: 0,
                identity_nonce_finalized: false,
                identity_user_abandoned: false,
            }
        })
        .collect()
}

/// The serialized transition of spend `n`: 1 a shielded transfer, 2 an
/// unshield to a Core script, one action per nullifier.
fn transition(n: u8, anchor: [u8; 32], nullifiers: &[[u8; 32]]) -> Vec<u8> {
    use dpp::identity::core_script::CoreScript;
    use dpp::serialization::PlatformSerializable;
    use dpp::shielded::SerializedAction;
    use dpp::state_transition::StateTransition;
    use dpp::state_transition::shielded_transfer_transition::ShieldedTransferTransition;
    use dpp::state_transition::shielded_transfer_transition::v0::ShieldedTransferTransitionV0;
    use dpp::state_transition::shielded_withdrawal_transition::ShieldedWithdrawalTransition;
    use dpp::state_transition::shielded_withdrawal_transition::v0::ShieldedWithdrawalTransitionV0;
    let actions = (nullifiers.iter())
        .map(|nullifier| SerializedAction {
            nullifier: *nullifier,
            rk: [n; 32],
            cmx: [0x30 + n; 32],
            encrypted_note: vec![0x40 + n; 216],
            cv_net: [0x50 + n; 32],
            spend_auth_sig: [0x60 + n; 64],
        })
        .collect();
    let st = match n {
        1 => StateTransition::ShieldedTransfer(ShieldedTransferTransition::V0(
            ShieldedTransferTransitionV0 {
                actions,
                value_balance: 1_000,
                anchor,
                proof: vec![0x70; 512],
                binding_signature: [0x71; 64],
            },
        )),
        _ => StateTransition::ShieldedWithdrawal(ShieldedWithdrawalTransition::V0(
            ShieldedWithdrawalTransitionV0 {
                actions,
                unshielding_amount: 100_000_000,
                anchor,
                proof: vec![0x72; 512],
                binding_signature: [0x73; 64],
                core_fee_per_byte: 1,
                pooling: dpp::withdrawal::Pooling::Never,
                output_script: CoreScript::from_bytes(vec![0x76, 0xa9, 0x14]),
            },
        )),
    };
    st.serialize_to_bytes().unwrap()
}

/// What the library's re-send does first with a stored transition
/// (operations.rs:3243, `StateTransition::deserialize_from_bytes_trusted`;
/// a failure there clears the record as undecodable): it decodes, to a
/// transfer or an unshield spending the record's nullifiers against its
/// anchor, and encodes back to the same bytes.
fn assert_redrivable(redrive: &PendingRedrive) {
    use dpp::serialization::{PlatformDeserializableTrusted, PlatformSerializable};
    use dpp::state_transition::StateTransition;
    let st = StateTransition::deserialize_from_bytes_trusted(&redrive.st_bytes)
        .expect("the library would clear an undecodable record");
    let (actions, anchor) = match &st {
        StateTransition::ShieldedTransfer(t) => {
            let dpp::state_transition::shielded_transfer_transition::ShieldedTransferTransition::V0(
                v,
            ) = t;
            (&v.actions, v.anchor)
        }
        StateTransition::ShieldedWithdrawal(t) => {
            let dpp::state_transition::shielded_withdrawal_transition::ShieldedWithdrawalTransition::V0(v) = t;
            (&v.actions, v.anchor)
        }
        other => panic!("not a spend: {}", other.name()),
    };
    let spent: Vec<[u8; 32]> = actions.iter().map(|a| a.nullifier).collect();
    assert_eq!(spent, redrive.nullifiers);
    assert_eq!(anchor, redrive.anchor);
    assert_eq!(st.serialize_to_bytes().unwrap(), redrive.st_bytes);
}

fn subwallet(wallet: WalletId) -> SubwalletId {
    SubwalletId::new(wallet.0, dw_vault::SHIELDED_ACCOUNT)
}

/// Stores [`spends`] for `wallet` as the library's spend calls do.
async fn stash_spends(s: &Arc<NetworkSession>, wallet: WalletId) {
    let m = s.manager().unwrap();
    let coordinator = pool::coordinator(s, &m, None).await.unwrap();
    let mut store = coordinator.store().write().await;
    for r in spends() {
        for n in &r.nullifiers {
            assert!(store.mark_pending(subwallet(wallet), n).unwrap());
            (store.set_pending_spend(subwallet(wallet), n, r.anchor, r.activity_id)).unwrap();
        }
        store.arm_redrive(subwallet(wallet), r).unwrap();
    }
}

/// Stores [`spends`] for `wallet` and puts them on record in the dispatch
/// journal (a pass's admission, with the background lease).
fn store_spends(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    engine.block_on(async move {
        stash_spends(&s, wallet).await;
        s.ensure_background().await;
        assert!(pool::admit_pass(&s, &m).await.is_ok());
    });
}

/// What must outlive the close: the stored transitions, their
/// reservations, the journal's records and the facade's pending rows.
#[derive(Debug, PartialEq)]
struct Kept {
    redrives: Vec<PendingRedrive>,
    reserved: Vec<StalePendingSpend>,
    audit: Vec<([u8; 32], [u8; 32])>,
    unresolved: Vec<UnresolvedSpend>,
}

fn kept(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> Kept {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    let (mut redrives, mut reserved) = engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let store = coordinator.store().read().await;
        (
            store.pending_redrives(subwallet(wallet)).unwrap(),
            store.stale_pending_spends(subwallet(wallet)).unwrap(),
        )
    });
    redrives.sort_by_key(|r| r.activity_id);
    reserved.sort();
    let mut audit = engine
        .block_on(session.leases.shielded_redrives(wallet))
        .unwrap();
    audit.sort();
    let mut unresolved = engine
        .block_on(on_pool(session, wallet, |_, lib| async move {
            Ok(pool::ShieldedLib::unresolved(&lib).await?)
        }))
        .unwrap();
    unresolved.sort_by(|a, b| a.activity_id.cmp(&b.activity_id));
    Kept {
        redrives,
        reserved,
        audit,
        unresolved,
    }
}

/// [`Kept`] right after [`store_spends`].
fn expected() -> Kept {
    let redrives = spends();
    let mut reserved: Vec<StalePendingSpend> = (redrives.iter())
        .flat_map(|r| (r.nullifiers.iter()).map(|n| (*n, r.anchor, Some(r.activity_id))))
        .collect();
    reserved.sort();
    let hash = |st: &[u8]| -> [u8; 32] { Sha256::digest(st).into() };
    Kept {
        audit: (redrives.iter())
            .map(|r| (r.activity_id, hash(&r.st_bytes)))
            .collect(),
        unresolved: (redrives.iter())
            .map(|r| UnresolvedSpend {
                activity_id: hex::encode(r.activity_id),
                transition: hex::encode(hash(&r.st_bytes)),
                attempts: 0,
            })
            .collect(),
        reserved,
        redrives,
    }
}

/// Close Wallet and an in-app close (the network switch's) are refused
/// while a spend is unresolved, and leave everything of it as it was; so
/// does any removal but the user's. Once the scan resolves the spends,
/// both go ahead.
#[test]
fn close_wallet_and_the_network_close_are_refused_while_a_spend_is_unresolved() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    store_spends(&engine, &session, wallet);
    assert_eq!(kept(&engine, &session, wallet), expected());

    let unloaded = engine.block_on(session.unload_wallet(wallet));
    assert!(
        matches!(unloaded, Err(EngineError::ShieldedSpendUnresolved)),
        "{unloaded:?}"
    );
    assert_eq!(
        unloaded.unwrap_err().to_string(),
        "a shielded payment is still being confirmed; wait, or quit and reopen the app"
    );
    assert!(session.require_wallet(&wallet).is_ok(), "still loaded");
    let closed = engine.block_on(engine.close_network(DashNetwork::Regtest));
    assert!(
        matches!(closed, Err(EngineError::ShieldedSpendUnresolved)),
        "{closed:?}"
    );
    assert!(
        engine
            .block_on(engine.session(&DashNetwork::Regtest))
            .is_some()
    );
    assert!(session.live().is_ok(), "the session stays open");
    // The restore's rollback takes the same refusal.
    let manager = session.manager().unwrap();
    let s = Arc::clone(&session);
    let removed = engine.block_on(async move {
        (s.platform
            .remove_from_manager(&s.leases, &manager, wallet, StoredSpends::Refuse))
        .await
    });
    assert!(matches!(removed, Err(EngineError::ShieldedSpendUnresolved)));
    assert_eq!(kept(&engine, &session, wallet), expected());

    // The scan finds both spent: nothing is held any more.
    let (s, m) = (Arc::clone(&session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let mut store = coordinator.store().write().await;
        for r in spends() {
            for n in &r.nullifiers {
                store.mark_spent(subwallet(wallet), n).unwrap();
            }
            store
                .clear_redrive(subwallet(wallet), &r.activity_id)
                .unwrap();
        }
    });
    engine.block_on(session.unload_wallet(wallet)).unwrap();
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
}

/// Own review r1, minors 1-3: a shielded call of the wallet in flight (here
/// its guards, as a send holds them from its quote to its record: the close
/// preflight's read side, then the wallet's lock) refuses the network close, and Close Wallet waits for it to end; a spend it stored
/// meanwhile then refuses the unload, which leaves the wallet loaded with
/// its background lease (the passes are not held back).
#[test]
fn a_close_waits_for_or_refuses_a_shielded_call_in_flight() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    assert!(bind(&engine, &session, wallet));
    engine.block_on(session.ensure_background());
    assert!(session.leases.background(&wallet).is_some());

    let signing = engine.block_on(session.platform.close_preflight.read());
    let spending = engine.block_on(pool::wallet_lock(&session, wallet).lock_owned());
    let closed = engine.block_on(engine.close_network(DashNetwork::Regtest));
    assert!(
        matches!(closed, Err(EngineError::ShieldedSpendUnresolved)),
        "{closed:?}"
    );
    let (tx, rx) = std::sync::mpsc::channel();
    let s = Arc::clone(&session);
    session.rt.spawn(async move {
        let _ = tx.send(s.unload_wallet(wallet).await);
    });
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "Close Wallet did not wait for the call"
    );
    // The call stores its spend, then ends.
    store_spends(&engine, &session, wallet);
    drop(spending);
    drop(signing);
    let unloaded = rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(
        matches!(unloaded, Err(EngineError::ShieldedSpendUnresolved)),
        "{unloaded:?}"
    );
    assert!(session.require_wallet(&wallet).is_ok(), "still loaded");
    assert_eq!(kept(&engine, &session, wallet), expected());
    assert!(session.leases.background(&wallet).is_some());
}

/// E1 r2 R2-3: an in-app close (a network switch's) checks the wallets,
/// then commits; no signing shielded call may start in between. With two
/// wallets the close pauses after its first wallet's check, and a send of
/// that wallet starts then (a body that stores spends as the library's
/// send does). It waits for the close and then finds the session closed:
/// the close reports success with nothing left unresolved, the network
/// reopens in this process (no wallet was kept loaded), and no spend was
/// stored. A close refused instead would have to keep the session open
/// with the spend; either way no switch reports success over a spend.
#[test]
fn a_spend_started_during_the_close_check_waits_and_finds_the_session_closed() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, first) = first_launch(&engine);
    let second = engine
        .block_on(session.import_wallet(
            Zeroizing::new(OTHER_PHRASE.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                core_compat: false,
                ..Default::default()
            },
        ))
        .unwrap();
    let (reached_tx, reached) = tokio::sync::oneshot::channel();
    let (resume, resume_rx) = tokio::sync::oneshot::channel();
    *session.platform.close_check_pause.lock().unwrap() = Some(pool::CheckPause {
        reached: reached_tx,
        resume: resume_rx,
    });
    let ran = Arc::new(AtomicBool::new(false));
    let (closed, called) = std::thread::scope(|scope| {
        let closing = scope.spawn(|| engine.block_on(engine.close_network(DashNetwork::Regtest)));
        let checked = engine.block_on(reached).unwrap();
        assert!([first, second].contains(&checked));
        // The checked wallet's send starts.
        let (done, done_rx) = std::sync::mpsc::channel();
        let (s, r) = (Arc::clone(&session), Arc::clone(&ran));
        session.rt.spawn(async move {
            let called = pool::on_pool_signing(&s, checked, move |s, _| async move {
                r.store(true, Ordering::SeqCst);
                stash_spends(&s, checked).await;
                Ok(())
            })
            .await;
            let _ = done.send(called);
        });
        // Long enough for the send to run, were it admitted.
        let early = done_rx.recv_timeout(Duration::from_secs(3)).ok();
        resume.send(()).unwrap();
        let closed = closing.join().unwrap();
        let called =
            early.unwrap_or_else(|| done_rx.recv_timeout(Duration::from_secs(30)).unwrap());
        (closed, called)
    });
    assert!(matches!(closed, Ok(true)), "{closed:?}");
    assert!(
        !ran.load(Ordering::SeqCst),
        "no spend started after the check"
    );
    assert!(
        called.is_err(),
        "the send found the session closed: {called:?}"
    );

    // Nothing unresolved was left behind, and the network reopens.
    let session = open(&engine);
    let (s, m) = (Arc::clone(&session), session.manager().unwrap());
    let stored = engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let store = coordinator.store().read().await;
        let mut stored = store.pending_redrives(subwallet(first)).unwrap();
        stored.extend(store.pending_redrives(subwallet(second)).unwrap());
        stored
    });
    assert!(stored.is_empty(), "{stored:?}");
}

/// E1 r3 m1: the shielded coordinator's first configuration rehydrates
/// every stored spend, so it is admitted as a wallet load is. Between the
/// close's check and its commit (here paused after the wallet's check) a
/// configuration, as the supervisor's start or a status call would run it,
/// is refused as `network_not_open` and leaves the coordinator
/// unconfigured; the close then commits with nothing kept, and the network
/// reopens in this process.
#[test]
fn a_coordinator_setup_during_the_close_check_is_refused() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    // A wallet no shielded call has used yet: the coordinator is not
    // configured (a bind would configure it).
    let session = open(&engine);
    engine
        .block_on(session.vault_op(|v| v.create(Some(PASS))))
        .unwrap();
    let wallet = engine
        .block_on(session.import_wallet(
            Zeroizing::new(PHRASE.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                core_compat: false,
                ..Default::default()
            },
        ))
        .unwrap();
    let manager = session.manager().unwrap();
    assert!(
        engine.block_on(manager.shielded_coordinator()).is_none(),
        "the coordinator opens on first use"
    );
    let (reached_tx, reached) = tokio::sync::oneshot::channel();
    let (resume, resume_rx) = tokio::sync::oneshot::channel();
    *session.platform.close_check_pause.lock().unwrap() = Some(pool::CheckPause {
        reached: reached_tx,
        resume: resume_rx,
    });
    let (closed, configured) = std::thread::scope(|scope| {
        let closing = scope.spawn(|| engine.block_on(engine.close_network(DashNetwork::Regtest)));
        assert_eq!(engine.block_on(reached).unwrap(), wallet);
        let (s, m) = (Arc::clone(&session), Arc::clone(&manager));
        // A close that is no switch's does not keep it waiting.
        let configured = engine.block_on(async move {
            tokio::time::timeout(Duration::from_secs(10), pool::coordinator(&s, &m, None))
                .await
                .map(|open| open.map(drop))
        });
        resume.send(()).unwrap();
        (closing.join().unwrap(), configured)
    });
    assert!(
        matches!(
            configured,
            Ok(Err(super::errors::PlatformError::NetworkNotOpen))
        ),
        "{configured:?}"
    );
    assert!(engine.block_on(manager.shielded_coordinator()).is_none());
    assert!(matches!(closed, Ok(true)), "{closed:?}");
    drop(manager);
    let session = open(&engine);
    assert!(session.require_wallet(&wallet).is_ok());
}

/// E1 r2, the owner on checkpoint 2: a wallet load in flight is no
/// shielded payment. An in-app close waits for it (here its admission,
/// held as a load holds it) up to `CLOSE_LOAD_WAIT`, then goes ahead once
/// it ends; past the bound it is refused as `wallet`, "a wallet is still
/// opening", the session left open, never `shielded_spend_unresolved`.
/// The wait holds no engine-wide lock (review m4), and a quit during it
/// leaves the close nothing to close.
#[test]
fn a_close_waits_for_a_wallet_load_and_never_calls_it_a_payment() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, _) = first_launch(&engine);
    let close = || engine.block_on(engine.close_network(DashNetwork::Regtest));

    // A load longer than the bound.
    let loading = engine.block_on(session.platform.wallet_loads.read());
    let closed = close();
    drop(loading);
    match &closed {
        Err(e @ EngineError::Wallet(_)) => {
            assert_eq!(e.code(), "wallet");
            assert!(e.to_string().contains("a wallet is still opening"), "{e}");
        }
        other => panic!("{other:?}"),
    }
    assert!(session.live().is_ok(), "the session stays open");

    // A load that ends within it: the close waits, then goes ahead.
    let loading = engine.block_on(session.platform.wallet_loads.read());
    let closed = std::thread::scope(|scope| {
        let closing = scope.spawn(close);
        std::thread::sleep(Duration::from_millis(300));
        assert!(!closing.is_finished(), "the close waits for the load");
        // The wait holds no engine-wide lock: lookups (and opens, quit)
        // go on meanwhile.
        let (found, looked_up) = std::sync::mpsc::channel();
        let engine = &engine;
        scope.spawn(move || {
            let _ = found.send(
                engine
                    .block_on(engine.session(&DashNetwork::Regtest))
                    .is_some(),
            );
        });
        assert_eq!(
            looked_up.recv_timeout(Duration::from_secs(1)),
            Ok(true),
            "the lookup waited for the close"
        );
        drop(loading);
        closing.join().unwrap()
    });
    assert!(matches!(closed, Ok(true)), "{closed:?}");

    // A quit during the wait: the close finds nothing left to close.
    let session = open(&engine);
    let loading = engine.block_on(session.platform.wallet_loads.read());
    let closed = std::thread::scope(|scope| {
        let closing = scope.spawn(close);
        std::thread::sleep(Duration::from_millis(300));
        engine.block_on(engine.shutdown()).unwrap();
        drop(loading);
        closing.join().unwrap()
    });
    assert!(matches!(closed, Ok(false)), "{closed:?}");
}

/// The quitting half of the next test, run in a child process: the first
/// launch stores the spends and issues a receive address, then quits
/// (`Engine::shutdown`, the app's quit) and exits at once, so nothing is
/// dropped after the quit (the kill-after-quit check of DEC-1046 (b)).
#[test]
#[ignore = "the child process of a_quit_keeps_unresolved_spends_for_the_next_launch"]
fn quit_with_unresolved_spends_child() {
    let Some(root) = std::env::var_os(QUIT_ROOT) else {
        return;
    };
    let engine = engine_at(Path::new(&root));
    let (session, wallet) = first_launch(&engine);
    store_spends(&engine, &session, wallet);
    let address = engine
        .block_on(session.next_receive_address(wallet, Some("before the quit".into())))
        .unwrap();
    std::fs::write(Path::new(&root).join("address"), &address.address).unwrap();
    std::fs::write(Path::new(&root).join("wallet"), wallet.to_string()).unwrap();
    engine.block_on(engine.shutdown()).unwrap();
    std::process::exit(0);
}

/// DEC-1046 (a) and (b): a quit with two unresolved spends (a transfer
/// and an unshield), then a relaunch. The stored transitions are byte for
/// byte the ones signed, their nullifiers are reserved again, the journal
/// still names them by hash, the facade lists them pending, and the first
/// admitted pass hands the library those identical bytes to re-send
/// (`redrive_pending_spends`, operations.rs:3212-3257, deserializes
/// `st_bytes` and broadcasts them). The re-send (coordinator.rs:1431)
/// comes after the pass scanned the notes (coordinator.rs:1381-1391),
/// which no local transport can answer, so
/// the test stops at the re-send's first step: each record decodes as
/// operations.rs:3243 does, to the signed transfer or unshield of its
/// nullifiers, and a real (offline) pass leaves every record, reservation
/// and attempt count as it was; nothing is signed or selected again.
/// What the quit left is committed in every store and outlives the
/// quitting process, which exited without dropping anything: the viewing
/// key in wallet.sqlite (bound again with the vault locked), the address
/// book in app.sqlite, the spends in the shielded store and their records
/// in the dispatch journal.
#[test]
fn a_quit_keeps_unresolved_spends_for_the_next_launch() {
    let dir = dw_testutil::private_tempdir();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "platform::shielded_close_tests::quit_with_unresolved_spends_child",
            "--exact",
            "--ignored",
            "--test-threads=1",
        ])
        .env(QUIT_ROOT, dir.path())
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "the quitting process failed: {}",
        String::from_utf8_lossy(&child.stderr)
    );
    let address = std::fs::read_to_string(dir.path().join("address")).unwrap();

    let engine = engine_at(dir.path());
    let session = open(&engine);
    let wallet: WalletId = (std::fs::read_to_string(dir.path().join("wallet")).unwrap())
        .parse()
        .unwrap();
    assert!(session.vault.lock_state() != dw_vault::LockState::Unlocked);
    assert!(bind(&engine, &session, wallet), "bound from wallet.sqlite");
    let book = engine
        .block_on(session.address_book(wallet, Some(crate::BookPurpose::Receive), None))
        .unwrap();
    assert!(book.iter().any(|e| e.address == address), "{book:?}");
    engine
        .block_on(session.vault_op(|v| v.unlock(PASS, UnlockScope::Full)))
        .unwrap();
    assert_eq!(kept(&engine, &session, wallet), expected());

    // The first pass admitted after the relaunch carries the same bytes.
    let (s, m) = (Arc::clone(&session), session.manager().unwrap());
    engine.block_on(async move {
        s.ensure_background().await;
        let coordinator = pool::admit_pass(&s, &m).await.unwrap();
        let mut stored = (coordinator.store().read().await)
            .pending_redrives_for_wallet(wallet.0)
            .unwrap();
        stored.sort_by_key(|(_, r)| r.activity_id);
        let stored: Vec<PendingRedrive> = stored.into_iter().map(|(_, r)| r).collect();
        assert_eq!(stored, spends());
        stored.iter().for_each(assert_redrivable);
        // A real pass (offline here: the anchors' prefetch gives none,
        // coordinator.rs:1369, and the note scan fails, so the pass ends
        // at :1391, before the re-send at :1431) leaves them as they were.
        assert!(pool::guarded_pass(&s, &m, true).await, "the pass ran");
    });
    assert_eq!(kept(&engine, &session, wallet), expected());
}

/// DEC-1057: the marker of a spend whose record the library lost: its
/// activity, anchor and two nullifiers.
const MARKED: [u8; 32] = [9; 32];
const MARKED_NULLIFIERS: [[u8; 32]; 2] = [[41; 32], [42; 32]];

/// Writes [`MARKED`] for `wallet` as a spend whose library record failed
/// does (`shielded_pool::mark_unrecorded`).
fn mark(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) {
    engine
        .block_on(session.leases.record_unrecorded_spend(
            wallet,
            MARKED,
            [7; 32],
            MARKED_NULLIFIERS.to_vec(),
        ))
        .unwrap();
}

fn markers(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> Vec<[u8; 32]> {
    (engine
        .block_on(session.leases.unrecorded_spends(wallet))
        .unwrap())
    .into_iter()
    .map(|m| m.activity)
    .collect()
}

/// What the library's persister receives from a proved scan (sync.rs:761-777)
/// or a spend's proved result (operations.rs:2847-2854): `nullifiers` of
/// `wallet`'s account seen spent, through the session's store, the tap.
fn spent_seen(session: &Arc<NetworkSession>, wallet: WalletId, nullifiers: &[[u8; 32]]) {
    use platform_wallet::changeset::{
        PlatformWalletChangeSet, PlatformWalletPersistence, ShieldedChangeSet,
    };
    let mut shielded = ShieldedChangeSet::default();
    shielded
        .nullifiers_spent
        .insert(subwallet(wallet), nullifiers.to_vec());
    let changeset = PlatformWalletChangeSet {
        shielded: Some(shielded),
        ..Default::default()
    };
    (session.live().unwrap().store)
        .store(wallet.0, changeset)
        .unwrap();
}

/// DEC-1057: a marked spend refuses Close Wallet, the network close and
/// the restore's rollback as a stored one does. Its nullifiers seen spent
/// one at a time through the store's tap: the first leaves it (a partial
/// set clears nothing), and so does another wallet's report of both; the
/// last clears it, and the wallet closes.
#[test]
fn a_marked_spend_refuses_the_close_until_a_scan_finds_all_its_notes_spent() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    mark(&engine, &session, wallet);
    assert_eq!(markers(&engine, &session, wallet), [MARKED]);

    let unloaded = engine.block_on(session.unload_wallet(wallet));
    assert!(
        matches!(unloaded, Err(EngineError::ShieldedSpendUnresolved)),
        "{unloaded:?}"
    );
    assert!(session.require_wallet(&wallet).is_ok(), "still loaded");
    let closed = engine.block_on(engine.close_network(DashNetwork::Regtest));
    assert!(
        matches!(closed, Err(EngineError::ShieldedSpendUnresolved)),
        "{closed:?}"
    );
    assert!(session.live().is_ok(), "the session stays open");
    let manager = session.manager().unwrap();
    let s = Arc::clone(&session);
    let removed = engine.block_on(async move {
        (s.platform
            .remove_from_manager(&s.leases, &manager, wallet, StoredSpends::Refuse))
        .await
    });
    assert!(
        matches!(removed, Err(EngineError::ShieldedSpendUnresolved)),
        "{removed:?}"
    );

    spent_seen(&session, wallet, &MARKED_NULLIFIERS[..1]);
    assert_eq!(markers(&engine, &session, wallet), [MARKED], "partial");
    let other = WalletId([3; 32]);
    let mut changeset_of_other = MARKED_NULLIFIERS.to_vec();
    changeset_of_other.push([43; 32]);
    {
        use platform_wallet::changeset::{
            PlatformWalletChangeSet, PlatformWalletPersistence, ShieldedChangeSet,
        };
        // Keyed by the other wallet's account: not this wallet's notes.
        let mut shielded = ShieldedChangeSet::default();
        shielded
            .nullifiers_spent
            .insert(subwallet(other), changeset_of_other);
        let changeset = PlatformWalletChangeSet {
            shielded: Some(shielded),
            ..Default::default()
        };
        let _ = (session.live().unwrap().store).store(wallet.0, changeset);
    }
    assert_eq!(markers(&engine, &session, wallet), [MARKED], "another's");
    spent_seen(&session, wallet, &MARKED_NULLIFIERS[1..]);
    assert!(markers(&engine, &session, wallet).is_empty(), "all spent");

    engine.block_on(session.unload_wallet(wallet)).unwrap();
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
}

/// DEC-1057: the user's removal (under its wipe grant) deletes the
/// wallet's markers with the wallet.
#[test]
fn the_users_removal_purges_a_marked_spend() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    mark(&engine, &session, wallet);
    let wipe = (session.vault).authorize(
        dw_vault::GrantPurpose::Wipe,
        Some(&wallet.0),
        dw_vault::Credential::Passphrase(PASS),
    );
    let wipe = wipe.unwrap().id;
    engine
        .block_on(session.remove_wallet(wallet, wipe))
        .unwrap();
    assert!(markers(&engine, &session, wallet).is_empty());
}

/// The quitting half of the next test, run in a child process: the first
/// launch marks a spend (DEC-1057), then quits and exits at once.
#[test]
#[ignore = "the child process of a_marked_spend_needs_a_confirmation_after_a_relaunch"]
fn quit_with_a_marked_spend_child() {
    let Some(root) = std::env::var_os(QUIT_ROOT) else {
        return;
    };
    let engine = engine_at(Path::new(&root));
    let (session, wallet) = first_launch(&engine);
    mark(&engine, &session, wallet);
    std::fs::write(Path::new(&root).join("wallet"), wallet.to_string()).unwrap();
    engine.block_on(engine.shutdown()).unwrap();
    std::process::exit(0);
}

/// DEC-1057 after a relaunch: the marker outlived the quitting process.
/// The status names it; Close Wallet is refused; a send and an unshield
/// are refused with `shielded.unrecorded_spend` unless confirmed, before
/// any quote (here the quotes' own refusals, an unsynced account's and an
/// amount below the withdrawal minimum, show the confirmed calls got past
/// it); a shield is not gated.
#[test]
fn a_marked_spend_needs_a_confirmation_after_a_relaunch() {
    let dir = dw_testutil::private_tempdir();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "platform::shielded_close_tests::quit_with_a_marked_spend_child",
            "--exact",
            "--ignored",
            "--test-threads=1",
        ])
        .env(QUIT_ROOT, dir.path())
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "the quitting process failed: {}",
        String::from_utf8_lossy(&child.stderr)
    );
    let engine = engine_at(dir.path());
    let session = open(&engine);
    let wallet: WalletId = (std::fs::read_to_string(dir.path().join("wallet")).unwrap())
        .parse()
        .unwrap();
    engine
        .block_on(session.vault_op(|v| v.unlock(PASS, UnlockScope::Full)))
        .unwrap();
    assert!(bind(&engine, &session, wallet));
    let dp = session.dashpay(wallet);

    let status = engine.block_on(dp.shielded_status()).unwrap();
    assert_eq!(
        status.unrecorded_spends,
        [super::shielded::UnrecordedSpend {
            activity_id: hex::encode(MARKED),
            since: status.unrecorded_spends[0].since,
        }]
    );
    assert!(status.unrecorded_spends[0].since > 0);
    let unloaded = engine.block_on(session.unload_wallet(wallet));
    assert!(
        matches!(unloaded, Err(EngineError::ShieldedSpendUnresolved)),
        "{unloaded:?}"
    );

    let to = engine
        .block_on(dp.shielded_receive_address())
        .unwrap()
        .address;
    let code = |r: Result<super::shielded::ShieldedSpendOutcome, super::ShieldedError>| {
        r.unwrap_err().code()
    };
    for confirmed in [false, true] {
        let sent =
            engine.block_on(dp.shielded_send(to.clone(), 1_000, None, "g".into(), confirmed));
        let unshielded = engine.block_on(dp.unshield_to_core(1_000, None, "g".into(), confirmed));
        let want = if confirmed {
            // The quotes' own refusals.
            ("shielded.not_synced", "shielded.below_minimum")
        } else {
            ("shielded.unrecorded_spend", "shielded.unrecorded_spend")
        };
        assert_eq!((code(sent), code(unshielded)), want);
    }
    let shield = engine.block_on(dp.shield_quote(100_000)).unwrap_err();
    assert_ne!(shield.code(), "shielded.unrecorded_spend", "{shield:?}");
}

/// Row 74 r1: a network switch's close holds the load fence while it
/// drains, and may still be refused. A shielded store's first open meanwhile
/// waits for the decision instead of failing, and opens once the close is
/// refused (here: an operation still running when the drain ends).
#[test]
fn a_shielded_store_open_waits_out_a_switch_close_that_is_refused() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, _) = first_launch(&engine);
    let manager = session.manager().unwrap();
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    let (held_tx, held) = std::sync::mpsc::channel();
    let s = Arc::clone(&session);
    session.rt.spawn(async move {
        let _op = s.enter().await.unwrap();
        held_tx.send(()).unwrap();
        let _ = released.await;
    });
    held.recv().unwrap();
    let (closed, configured) = std::thread::scope(|scope| {
        let closing = scope.spawn(|| {
            engine.block_on(engine.close_network_unless_busy(
                DashNetwork::Regtest,
                Duration::from_millis(500),
                |_| None::<()>,
            ))
        });
        for _ in 0..2_000 {
            if session.gate.drains() > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(session.gate.drains(), 1, "the close drains");
        let (s, m) = (Arc::clone(&session), Arc::clone(&manager));
        let configured =
            engine.block_on(async move { pool::coordinator(&s, &m, None).await.map(drop) });
        (closing.join().unwrap(), configured)
    });
    assert!(
        matches!(closed, Err(crate::CloseRefused::Draining)),
        "{closed:?}"
    );
    assert!(configured.is_ok(), "{configured:?}");
    assert!(engine.block_on(manager.shielded_coordinator()).is_some());
    drop(release);
    assert!(session.is_open());
}

/// Holds an operation guard of `session` until the returned sender drops.
fn hold_operation(session: &Arc<NetworkSession>) -> tokio::sync::oneshot::Sender<()> {
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    let (held_tx, held) = std::sync::mpsc::channel();
    let s = Arc::clone(session);
    session.rt.spawn(async move {
        let _op = s.enter().await.unwrap();
        held_tx.send(()).unwrap();
        let _ = released.await;
    });
    held.recv().unwrap();
    release
}

fn switch_close(engine: &Engine, drain: Duration) -> Result<bool, crate::CloseRefused<()>> {
    engine.block_on(engine.close_network_unless_busy(DashNetwork::Regtest, drain, |_| None::<()>))
}

fn wait_until(what: impl Fn() -> bool, why: &str) {
    for _ in 0..4_000 {
        if what() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("{why}");
}

/// Row 74 r2 (test gap): a signing shielded call running refuses the
/// switch's close before it pauses the gate, without a drain.
#[test]
fn a_shielded_call_running_refuses_the_switch_before_its_drain() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, _) = first_launch(&engine);
    let signing = engine.block_on(session.platform.close_preflight.read());
    let _op = hold_operation(&session);
    let closed = switch_close(&engine, Duration::from_millis(300));
    assert!(
        matches!(
            closed,
            Err(crate::CloseRefused::Engine(
                EngineError::ShieldedSpendUnresolved
            ))
        ),
        "{closed:?}"
    );
    assert_eq!(session.gate.drains(), 0, "refused before the drain");
    drop(signing);
    assert!(session.is_open());
}

/// Row 74 r2 (test gap): a signing shielded call that starts while the
/// switch drains (it holds the close preflight, then waits on the pause)
/// names the refusal once the drain runs out.
#[test]
fn a_shielded_call_starting_during_the_drain_names_the_refusal() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, _) = first_launch(&engine);
    let op = hold_operation(&session);
    let (closed, signing) = std::thread::scope(|scope| {
        let closing = scope.spawn(|| switch_close(&engine, Duration::from_millis(500)));
        wait_until(|| session.gate.drains() == 1, "the close never drained");
        let signing = engine.block_on(session.platform.close_preflight.read());
        (closing.join().unwrap(), signing)
    });
    assert!(
        matches!(
            closed,
            Err(crate::CloseRefused::Engine(
                EngineError::ShieldedSpendUnresolved
            ))
        ),
        "{closed:?}"
    );
    drop((signing, op));
    assert!(session.is_open());
}

/// Row 74 r2 (test gap): a refused switch's close releases its load fence
/// before it resumes the gate. A shielded store's first open at that point
/// finds the fence free and opens.
#[test]
fn a_refused_switch_frees_the_load_fence_before_the_gate_resumes() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, _) = unbound_launch(&engine);
    let manager = session.manager().unwrap();
    let op = hold_operation(&session);
    let (opened_tx, opened) = std::sync::mpsc::channel();
    {
        let (s, m) = (Arc::clone(&session), Arc::clone(&manager));
        *session.gate.on_release.lock().unwrap() = Some(Box::new(move || {
            let (tx, rx) = std::sync::mpsc::channel();
            let rt = s.rt.clone();
            rt.spawn(async move {
                let _ = tx.send(pool::coordinator(&s, &m, None).await.map(drop));
            });
            let open = tokio::task::block_in_place(|| rx.recv_timeout(Duration::from_secs(30)));
            let _ = opened_tx.send(open);
        }));
    }
    let closed = switch_close(&engine, Duration::from_millis(300));
    assert!(
        matches!(closed, Err(crate::CloseRefused::Draining)),
        "{closed:?}"
    );
    let open = opened.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(matches!(open, Ok(Ok(()))), "{open:?}");
    drop(op);
    assert!(session.is_open());
}

/// Row 74 r2 m2: a switch's close waiting for a wallet load holds the gate
/// too. A shielded store's first open meanwhile waits for the decision, and
/// opens once the close is refused (`wallet`, the load still running).
#[test]
fn a_shielded_store_open_waits_out_a_close_refused_for_a_wallet_load() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, _) = unbound_launch(&engine);
    let manager = session.manager().unwrap();
    *session.close_load_wait.lock().unwrap() = Some(Duration::from_millis(500));
    let loading = engine.block_on(session.platform.wallet_loads.read());
    let (closed, opened) = std::thread::scope(|scope| {
        let closing = scope.spawn(|| switch_close(&engine, Duration::from_millis(300)));
        wait_until(|| session.gate.is_held(), "the close never held the gate");
        wait_until(
            || session.platform.wallet_loads.try_read().is_err(),
            "the close's write never queued behind the load",
        );
        let (s, m) = (Arc::clone(&session), Arc::clone(&manager));
        let opened =
            engine.block_on(async move { pool::coordinator(&s, &m, None).await.map(drop) });
        (closing.join().unwrap(), opened)
    });
    assert!(
        matches!(
            closed,
            Err(crate::CloseRefused::Engine(EngineError::Wallet(_)))
        ),
        "{closed:?}"
    );
    assert_eq!(session.gate.drains(), 0);
    assert!(opened.is_ok(), "{opened:?}");
    assert!(engine.block_on(manager.shielded_coordinator()).is_some());
    drop(loading);
    assert!(session.is_open());
}

/// Row 74 r2 M2: a quit while a switch's close holds the load fence and the
/// pause, with the Platform start waiting at its shielded store's first
/// open, ends. The start's cancellation ends that open, the quit closes
/// the session, and the switch then finds nothing left to close.
#[test]
fn a_quit_during_a_switch_ends_the_shielded_open_waiting_for_it() {
    use super::runtime::{Supervisor, guard};
    use super::shielded::SHIELDED_ACTIVATION_VERSION;
    use crate::coins::TestPause;
    const BOUND: Duration = Duration::from_secs(20);
    let dir = dw_testutil::private_tempdir();
    let engine = Arc::new(engine_at(dir.path()));
    let active = crate::SessionOptions {
        initial_protocol_version: Some(SHIELDED_ACTIVATION_VERSION),
        ..options()
    };
    let (session, _) = unbound_launch_with(&engine, active);
    let manager = session.manager().unwrap();
    assert!(manager.sdk_arc().protocol_version_number() >= SHIELDED_ACTIVATION_VERSION);

    // The Platform start's shielded step, held right before it begins.
    let before_open = Arc::new(TestPause::default());
    let (cancel, cancel_rx) = tokio::sync::watch::channel(false);
    let (signals, _signals) = tokio::sync::mpsc::unbounded_channel();
    let (opened_tx, opened) = std::sync::mpsc::channel();
    let task = {
        let (s, m, before_open) = (
            Arc::clone(&session),
            Arc::clone(&manager),
            Arc::clone(&before_open),
        );
        session.rt.spawn(async move {
            before_open
                .reached
                .store(true, std::sync::atomic::Ordering::SeqCst);
            before_open.release.notified().await;
            pool::start(&s, &m, &[], &cancel_rx).await;
            let _ = opened_tx.send(());
        })
    };
    *guard(&session.platform.supervisor) = Some(Supervisor {
        cancel,
        signals,
        task: Some(task),
        running: true,
    });
    wait_until(
        || {
            before_open
                .reached
                .load(std::sync::atomic::Ordering::SeqCst)
        },
        "the start never ran",
    );

    // The switch's close, checked, held right before the engine's sessions.
    let checked = Arc::new(TestPause::default());
    *session.gate.hold_before_sessions.lock().unwrap() = Some(Arc::clone(&checked));
    let (switched_tx, switched) = std::sync::mpsc::channel();
    {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || {
            // Long enough for the launch's own operations to end.
            let _ = switched_tx.send(switch_close(&engine, Duration::from_secs(10)));
        });
    }
    wait_until(
        || {
            if let Ok(switched) = switched.try_recv() {
                panic!("the switch ended before its check: {switched:?}");
            }
            checked.reached.load(std::sync::atomic::Ordering::SeqCst)
        },
        "the switch never checked",
    );
    before_open.release.notify_one();
    std::thread::sleep(Duration::from_millis(200));
    if let Ok(open) = opened.try_recv() {
        panic!("the open waits for the switch: {open:?}");
    }

    // Quit.
    let (quit_tx, quit) = std::sync::mpsc::channel();
    {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || {
            let _ = quit_tx.send(
                engine.block_on(async { tokio::time::timeout(BOUND, engine.shutdown()).await }),
            );
        });
    }
    let quit = quit
        .recv_timeout(BOUND + Duration::from_secs(10))
        .expect("the quit returns");
    assert!(matches!(quit, Ok(Ok(()))), "the quit did not end: {quit:?}");
    opened
        .recv_timeout(Duration::from_secs(5))
        .expect("the start ends");
    assert!(
        engine.block_on(manager.shielded_coordinator()).is_none(),
        "the store never opened"
    );
    assert!(!session.is_open());

    checked.release.notify_one();
    let switched = switched.recv_timeout(BOUND).expect("the switch returns");
    assert!(matches!(switched, Ok(false)), "{switched:?}");
}

/// The Platform start's first step, the invitation holds' settle, waits out
/// the switch's pause too: the quit's cancel ends the start.
#[test]
fn a_quit_during_a_switch_ends_the_invitation_settle_waiting_for_it() {
    use super::runtime::{Supervisor, guard};
    use crate::coins::TestPause;
    const BOUND: Duration = Duration::from_secs(20);
    let dir = dw_testutil::private_tempdir();
    let engine = Arc::new(engine_at(dir.path()));
    let (session, _) = first_launch(&engine);

    // The switch's close, checked, held right before the engine's sessions.
    let checked = Arc::new(TestPause::default());
    *session.gate.hold_before_sessions.lock().unwrap() = Some(Arc::clone(&checked));
    let (switched_tx, switched) = std::sync::mpsc::channel();
    {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || {
            // Long enough for the launch's own operations to end.
            let _ = switched_tx.send(switch_close(&engine, Duration::from_secs(10)));
        });
    }
    wait_until(
        || {
            if let Ok(switched) = switched.try_recv() {
                panic!("the switch ended before its check: {switched:?}");
            }
            checked.reached.load(std::sync::atomic::Ordering::SeqCst)
        },
        "the switch never checked",
    );

    // A Platform start begins under the pause.
    let manager = session.manager().unwrap();
    let (cancel, cancel_rx) = tokio::sync::watch::channel(false);
    let (signals, signals_rx) = tokio::sync::mpsc::unbounded_channel();
    let (ended_tx, ended) = std::sync::mpsc::channel();
    let task = {
        let s = Arc::clone(&session);
        session.rt.spawn(async move {
            s.supervise(
                manager,
                Vec::new(),
                (std::time::Instant::now(), 0),
                cancel_rx,
                signals_rx,
            )
            .await;
            let _ = ended_tx.send(());
        })
    };
    *guard(&session.platform.supervisor) = Some(Supervisor {
        cancel,
        signals,
        task: Some(task),
        running: true,
    });
    std::thread::sleep(Duration::from_millis(200));
    assert!(ended.try_recv().is_err(), "the start waits for the switch");

    // Quit.
    let (quit_tx, quit) = std::sync::mpsc::channel();
    {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || {
            let _ = quit_tx.send(
                engine.block_on(async { tokio::time::timeout(BOUND, engine.shutdown()).await }),
            );
        });
    }
    let quit = quit
        .recv_timeout(BOUND + Duration::from_secs(10))
        .expect("the quit returns");
    assert!(matches!(quit, Ok(Ok(()))), "the quit did not end: {quit:?}");
    ended
        .recv_timeout(Duration::from_secs(5))
        .expect("the start ends");
    assert!(!session.is_open());

    checked.release.notify_one();
    let switched = switched.recv_timeout(BOUND).expect("the switch returns");
    assert!(matches!(switched, Ok(false)), "{switched:?}");
}

/// Quits `engine` on another thread; the result within `bound`.
fn quit_within(
    engine: &Arc<Engine>,
    bound: Duration,
) -> std::sync::mpsc::Receiver<Result<Result<(), crate::EngineError>, tokio::time::error::Elapsed>>
{
    let (quit_tx, quit) = std::sync::mpsc::channel();
    let engine = Arc::clone(engine);
    std::thread::spawn(move || {
        let _ = quit_tx
            .send(engine.block_on(async { tokio::time::timeout(bound, engine.shutdown()).await }));
    });
    quit
}

/// Row 74 r3 M1: a quit while a switch drains, with a shielded quote's
/// nested protocol refresh waiting on the switch's pause, ends. The quit's
/// gate close refuses the refresh at once, so it never queues behind the
/// close's writer, which waits for the quote's own guard; the quote ends
/// with an error, and the switch then finds nothing left to close.
#[test]
fn a_quit_during_a_switch_drain_ends_a_quote_waiting_on_its_nested_refresh() {
    use crate::coins::TestPause;
    use std::sync::atomic::Ordering;
    const BOUND: Duration = Duration::from_secs(20);
    let dir = dw_testutil::private_tempdir();
    let engine = Arc::new(engine_at(dir.path()));
    let (session, wallet) = first_launch(&engine);

    // The quote, admitted, held right before its price's protocol refresh
    // asks the gate.
    let refresh = Arc::new(TestPause::default());
    *session.platform.hold_refresh.lock().unwrap() = Some(Arc::clone(&refresh));
    let (quoted_tx, quoted) = std::sync::mpsc::channel();
    {
        let dashpay = session.dashpay(wallet);
        session.rt.spawn(async move {
            let _ = quoted_tx.send(dashpay.shielded_send_quote("any".into(), 1_000).await);
        });
    }
    wait_until(
        || refresh.reached.load(Ordering::SeqCst),
        "the quote never reached its refresh",
    );

    // The switch pauses and drains, which the quote's guard keeps going.
    let (switched_tx, switched) = std::sync::mpsc::channel();
    {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || {
            let _ = switched_tx.send(switch_close(&engine, Duration::from_secs(10)));
        });
    }
    wait_until(|| session.gate.drains() == 1, "the switch never drained");
    // The refresh asks the gate and waits on the pause.
    let waiting = session.gate.waiting_on_pause();
    refresh.release.notify_one();
    wait_until(
        || session.gate.waiting_on_pause() > waiting,
        "the refresh never waited on the pause",
    );

    // Quit, while the switch still drains.
    let quit = quit_within(&engine, BOUND);
    wait_until(
        || session.gate.is_closed(),
        "the quit never closed the gate",
    );
    let quit = quit
        .recv_timeout(BOUND + Duration::from_secs(10))
        .expect("the quit returns");
    assert!(matches!(quit, Ok(Ok(()))), "the quit did not end: {quit:?}");
    let quoted = quoted
        .recv_timeout(Duration::from_secs(5))
        .expect("the quote ends");
    assert!(quoted.is_err(), "{quoted:?}");
    assert!(!session.is_open());
    let switched = switched.recv_timeout(BOUND).expect("the switch returns");
    assert!(matches!(switched, Ok(false)), "{switched:?}");
}

/// Row 74 r3 m1: a signing call that starts once a switch has drained
/// waits for the pause to end before it takes the close preflight. The
/// switch commits rather than refuse as an unresolved spend, and the call,
/// which never started, finds the session closed.
#[test]
fn a_signing_call_after_the_drain_lets_the_switch_commit() {
    use crate::coins::TestPause;
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    let checked = Arc::new(TestPause::default());
    *session.gate.hold_before_sessions.lock().unwrap() = Some(Arc::clone(&checked));
    let ran = Arc::new(AtomicBool::new(false));
    let (switched, called) = std::thread::scope(|scope| {
        let closing = scope.spawn(|| switch_close(&engine, Duration::from_secs(10)));
        wait_until(
            || checked.reached.load(Ordering::SeqCst),
            "the switch never checked",
        );
        let waiting = session.gate.waiting_on_pause();
        let (done, done_rx) = std::sync::mpsc::channel();
        let (s, r) = (Arc::clone(&session), Arc::clone(&ran));
        session.rt.spawn(async move {
            let called = pool::on_pool_signing(&s, wallet, move |_, _| async move {
                r.store(true, Ordering::SeqCst);
                Ok(())
            })
            .await;
            let _ = done.send(called);
        });
        wait_until(
            || session.gate.waiting_on_pause() > waiting,
            "the call never waited on the pause",
        );
        checked.release.notify_one();
        let switched = closing.join().unwrap();
        (
            switched,
            done_rx.recv_timeout(Duration::from_secs(30)).unwrap(),
        )
    });
    assert!(matches!(switched, Ok(true)), "{switched:?}");
    assert!(!ran.load(Ordering::SeqCst), "no spend started");
    assert!(
        matches!(
            called,
            Err(super::errors::ShieldedError::Platform(
                super::errors::PlatformError::NetworkNotOpen
            ))
        ),
        "the call found the session closed: {called:?}"
    );
}

/// Row 74 r3 (Opus N2): a shielded store's first open that found the load
/// fence taken, with the switch then refused in full (fence freed, gate let
/// go) before it looks at the hold, reads the fence again and opens. The
/// switch is refused as busy once the open has found the fence taken and
/// the operation it drains has ended.
#[test]
fn a_store_open_between_its_fence_reads_opens_after_the_refusal() {
    use crate::coins::TestPause;
    use std::sync::atomic::Ordering;
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, _) = unbound_launch(&engine);
    let manager = session.manager().unwrap();
    let op = hold_operation(&session);
    let between = Arc::new(TestPause::default());
    *session.platform.hold_between_fence_reads.lock().unwrap() = Some(Arc::clone(&between));
    let (opened_tx, opened) = std::sync::mpsc::channel();
    let closed = std::thread::scope(|scope| {
        let reached = Arc::clone(&between);
        let closing = scope.spawn(|| {
            engine.block_on(engine.close_network_unless_busy(
                DashNetwork::Regtest,
                Duration::from_secs(60),
                move |_| reached.reached.load(Ordering::SeqCst).then_some(()),
            ))
        });
        wait_until(|| session.gate.drains() == 1, "the close never drained");
        let (s, m) = (Arc::clone(&session), Arc::clone(&manager));
        session.rt.spawn(async move {
            let _ = opened_tx.send(pool::coordinator(&s, &m, None).await.map(drop));
        });
        wait_until(
            || between.reached.load(Ordering::SeqCst),
            "the open never found the fence taken",
        );
        drop(op);
        closing.join().unwrap()
    });
    assert!(
        matches!(closed, Err(crate::CloseRefused::Busy(()))),
        "{closed:?}"
    );
    assert!(!session.gate.is_held());
    between.release.notify_one();
    let open = opened.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(open.is_ok(), "{open:?}");
    assert!(engine.block_on(manager.shielded_coordinator()).is_some());
    assert!(session.is_open());
}

/// Row 74 r3 (Sol): the supervisor's cancel during an invitation-hold
/// settle already admitted ends only the supervisor's wait. The settle
/// keeps its guard, and a close waits for its vault work to end.
#[test]
fn a_cancel_during_an_admitted_settle_keeps_the_close_behind_it() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, _) = first_launch(&engine);
    let (reached_tx, reached) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    *session.platform.hold_settle_retain.lock().unwrap() = Some((reached_tx, release_rx));
    let (cancel, mut cancel_rx) = tokio::sync::watch::channel(false);
    let (settled_tx, settled) = std::sync::mpsc::channel();
    {
        let s = Arc::clone(&session);
        session.rt.spawn(async move {
            let _ = settled_tx.send(s.settle_holds_unless(&mut cancel_rx).await);
        });
    }
    reached
        .recv_timeout(Duration::from_secs(30))
        .expect("the settle never reached its vault work");
    wait_until(
        || session.gate.admitted() == 1,
        "the settle is not the one operation admitted",
    );
    cancel.send(true).unwrap();
    let settled = settled.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(!settled, "the cancel ends the supervisor's wait");
    assert_eq!(
        session.gate.admitted(),
        1,
        "the admitted settle keeps its guard"
    );
    let closed = std::thread::scope(|scope| {
        let closing = scope.spawn(|| engine.block_on(engine.close_network(DashNetwork::Regtest)));
        wait_until(
            || session.gate.is_closed(),
            "the close never reached the gate",
        );
        assert!(!closing.is_finished(), "the close waits for the settle");
        release.send(()).unwrap();
        closing.join().unwrap()
    });
    assert!(matches!(closed, Ok(true)), "{closed:?}");
}
