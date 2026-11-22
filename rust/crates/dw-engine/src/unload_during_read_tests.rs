//! #263: a wallet unloaded while a read of its coins waits (on an
//! app.sqlite job or its coin fence) ends that read with `WalletNotFound`,
//! never a panicking engine task.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dw_vault::{Credential, GrantPurpose};

use crate::held_writes::{ReleaseOnDrop, hold_first_job};
use crate::send::{CoinSource, FeeMode, Recipient};
use crate::{DashNetwork, Engine, EngineError, NetworkSession, WalletId};

const PHRASE: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const FOREIGN: &str = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n";

struct Silent;

impl crate::EventSink for Silent {
    fn emit(&self, _: crate::EngineEvent) {}
}

/// An offline regtest engine with one imported wallet.
fn engine_wallet() -> (tempfile::TempDir, Engine, Arc<NetworkSession>, WalletId) {
    use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
    let dir = dw_testutil::private_tempdir();
    let engine = Engine::new(
        crate::EngineConfig {
            data_root: dir.path().join("data"),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..Default::default()
            },
        },
        Arc::new(Silent),
    )
    .unwrap();
    let options = crate::SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        no_platform: true,
        ..Default::default()
    };
    let s = engine
        .block_on(engine.open_network(DashNetwork::Regtest, options))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let id = engine
        .block_on(s.import_wallet(
            zeroize::Zeroizing::new(PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(0),
                ..Default::default()
            },
        ))
        .unwrap();
    (dir, engine, s, id)
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Runs `read` with its first app.sqlite job held, unloads wallet `id`
/// meanwhile, then lets `read` go on; what it returned.
fn unloaded_during_appdb_job<T: Send + 'static>(
    engine: &Engine,
    s: &Arc<NetworkSession>,
    id: WalletId,
    read: impl Future<Output = Result<T, EngineError>> + Send + 'static,
) -> Result<T, EngineError> {
    let out = Arc::new(Mutex::new(None));
    let (pause, task) = hold_first_job(s, {
        let out = Arc::clone(&out);
        async move {
            let r = read.await;
            *out.lock().unwrap() = Some(r);
        }
    });
    let _release = ReleaseOnDrop(Arc::clone(&pause));
    wait_until("the held job", || pause.reached.load(Ordering::SeqCst));
    engine.block_on(s.unload_wallet(id)).unwrap();
    pause.release.notify_one();
    let joined = engine.block_on(task);
    assert!(
        !joined.as_ref().is_err_and(|e| e.is_panic()),
        "the read's task panicked"
    );
    out.lock().unwrap().take().expect("the read returned")
}

fn assert_gone<T: std::fmt::Debug>(r: Result<T, EngineError>) {
    match r {
        Err(EngineError::WalletNotFound(_)) => {}
        Err(e) if e.is_task_panic() => panic!("an engine task panicked"),
        other => panic!("expected WalletNotFound: {other:?}"),
    }
}

fn fully_mixed_draft(s: &Arc<NetworkSession>, id: WalletId) -> Arc<crate::send::TxDraft> {
    let draft = s.new_tx_draft(id).unwrap();
    draft
        .set_recipients(vec![Recipient {
            address: FOREIGN.into(),
            amount: 10_000_000,
            subtract_fee_from_amount: false,
            label: None,
            message: None,
        }])
        .unwrap();
    draft.set_source(CoinSource::FullyMixedOnly).unwrap();
    draft
}

#[test]
fn unloading_a_wallet_during_its_coin_control_list_does_not_panic() {
    let (_dir, engine, s, id) = engine_wallet();
    let read = {
        let s = Arc::clone(&s);
        async move { s.utxos(id, crate::CoinFilter::default()).await }
    };
    assert_gone(unloaded_during_appdb_job(&engine, &s, id, read));
}

#[test]
fn unloading_a_wallet_during_its_coinjoin_status_does_not_panic() {
    let (_dir, engine, s, id) = engine_wallet();
    let read = {
        let s = Arc::clone(&s);
        async move {
            let fence = s.coin_fence(id).await?;
            s.compute_coinjoin_status(&fence).await
        }
    };
    assert_gone(unloaded_during_appdb_job(&engine, &s, id, read));
}

#[test]
fn unloading_a_wallet_during_a_fully_mixed_estimate_does_not_panic() {
    let (_dir, engine, s, id) = engine_wallet();
    let draft = fully_mixed_draft(&s, id);
    let read = async move { draft.estimate().await };
    assert_gone(unloaded_during_appdb_job(&engine, &s, id, read));
}

/// A close before the hand-off leaves nothing of the payment: no sent mark,
/// no coin reservation, no contact lock.
#[test]
fn unloading_a_wallet_during_a_fully_mixed_prepare_leaves_nothing() {
    let (_dir, engine, s, id) = engine_wallet();
    let draft = fully_mixed_draft(&s, id);
    let grant = (s.vault())
        .authorize(
            GrantPurpose::Spend {
                max_duffs: 20_000_000,
            },
            Some(&id.0),
            Credential::None,
        )
        .unwrap()
        .id;
    let read = async move { draft.prepare(grant).await.map(drop) };
    assert_gone(unloaded_during_appdb_job(&engine, &s, id, read));
    let appdb = Arc::clone(&s.live().unwrap().appdb);
    assert!(
        crate::tx_actions::load_sent(&appdb, id).0.is_empty(),
        "a sent mark"
    );
    assert!(s.spends.snapshot(&id).is_empty(), "a reservation");
    let locks = appdb.dp_payment_locks(&id.to_string()).unwrap();
    assert!(locks.is_empty(), "a contact lock");
}

/// The coin read of a call that waited for the wallet's coin fence while
/// the wallet was unloaded (`coin_snapshot`'s first read).
#[test]
fn unloading_a_wallet_while_a_coin_read_waits_for_its_fence_does_not_panic() {
    let (_dir, engine, s, id) = engine_wallet();
    let fence = engine.block_on(s.coin_fence(id)).unwrap();
    let task = s.rt.spawn({
        let s = Arc::clone(&s);
        async move {
            s.coin_selection_summary(id, Vec::new(), Vec::new(), FeeMode::PerKb(1_000), false)
                .await
        }
    });
    std::thread::sleep(Duration::from_millis(500));
    engine.block_on(s.unload_wallet(id)).unwrap();
    drop(fence);
    let joined = engine.block_on(task);
    assert!(
        !joined.as_ref().is_err_and(|e| e.is_panic()),
        "the read's task panicked"
    );
    assert_gone(joined.unwrap());
}
