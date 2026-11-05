//! "Move mixed coins" and its consolidation (0.2 MIX-SHIELD unit A)
//! against a real session and vault: the mixing refusal, the consolidation
//! round inside the CoinJoin account, and what a restart keeps of a sweep
//! that may be on the network.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use dashcore::hashes::Hash;
use dashcore::{OutPoint, Transaction, Txid};
use dw_vault::{Credential, GrantPurpose, is_bip44_path, is_coinjoin_path};
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use platform_wallet::PlatformWalletError as Pw;

use crate::EngineError;
use crate::coinjoin::sweep_test_hooks::Hooks;
use crate::coinjoin::{
    CoinJoinFailure, SweepDestination, SweepResend, SweepResendState, SweepResult,
};
use crate::coins::TestPause;
use crate::history::TxEntry;
use crate::platform::lease::session_tests::{Fixture, fixture};
use crate::send::SpendOwner;
use dashcore::BlockHash;
use key_wallet::transaction_checking::{BlockInfo, TransactionContext};

const COIN: u64 = 100_000_000;

/// What the stand-in broadcast does with the n-th chunk it is handed.
#[derive(Clone, Copy)]
enum Outcome {
    Sent,
    Rejected,
    Unknown,
}

/// The chunks handed to the network, in order.
#[derive(Default)]
struct Wire {
    seen: Mutex<Vec<Transaction>>,
}

impl Wire {
    fn txs(&self) -> Vec<Transaction> {
        self.seen.lock().unwrap().clone()
    }
}

/// Installs a broadcast that answers `outcomes` in order (`Sent` past the
/// end) and chunks of at most `max_inputs`.
fn hook(f: &Fixture, max_inputs: usize, outcomes: &[Outcome]) -> Arc<Wire> {
    let wire = Arc::new(Wire::default());
    let (w, outcomes) = (Arc::clone(&wire), outcomes.to_vec());
    let broadcast: crate::coinjoin::sweep_test_hooks::Broadcast = Arc::new(move |tx| {
        let mut seen = w.seen.lock().unwrap();
        let n = seen.len();
        seen.push(tx.clone());
        match outcomes.get(n).copied().unwrap_or(Outcome::Sent) {
            Outcome::Sent => Ok(tx.txid()),
            Outcome::Rejected => Err(Box::new(Pw::TransactionBroadcast(
                "rejected: txn-mempool-conflict".into(),
            ))),
            Outcome::Unknown => Err(Box::new(Pw::TransactionBroadcastUnconfirmed(
                "no verdict".into(),
            ))),
        }
    });
    *f.session.coinjoin.sweep_hooks.lock().unwrap() = Hooks {
        broadcast: Some(broadcast),
        max_inputs: Some(max_inputs),
        ..Hooks::default()
    };
    wire
}

fn spend_grant(f: &Fixture) -> String {
    f.grant(
        GrantPurpose::Spend {
            max_duffs: 1_000 * COIN,
        },
        Credential::None,
    )
}

fn consolidate(f: &Fixture) -> Result<SweepResult, EngineError> {
    let grant = spend_grant(f);
    f.engine
        .block_on(f.session.consolidate_mixed_coins(f.wallet, grant))
}

fn move_to_wallet(f: &Fixture) -> Result<SweepResult, EngineError> {
    let grant = spend_grant(f);
    (f.engine).block_on(
        f.session
            .move_mixed_coins(f.wallet, SweepDestination::Wallet, grant),
    )
}

/// A restart: the session's in-memory reservations of `txs` are gone.
fn forget_claims(f: &Fixture, txs: &[Transaction]) {
    for tx in txs {
        f.session
            .spends
            .release(&f.wallet, SpendOwner::Tx(tx.txid()));
    }
    assert!(f.session.spends.snapshot(&f.wallet).is_empty());
}

/// SPV saw `txs`: their inputs are no coins of the wallet any more, and
/// the wallet's history holds them, in the mempool.
fn see_spent(f: &Fixture, txs: &[Transaction]) {
    drop_coins(f, &inputs(txs));
    for tx in txs {
        record_history(f, tx, None);
    }
}

/// The CoinJoin account no longer holds the coins `spent`.
fn drop_coins(f: &Fixture, spent: &HashSet<OutPoint>) {
    let spent = spent.clone();
    let session = Arc::clone(&f.session);
    let id = f.wallet;
    f.engine
        .block_on(session.on_runtime({
            let session = Arc::clone(&session);
            async move {
                let wallet = session.wallet(&id).await?;
                let mut state = wallet.state_mut().await;
                let account = (state.core_wallet.accounts.coinjoin_accounts)
                    .get_mut(&0)
                    .unwrap();
                account.utxos.retain(|o, _| !spent.contains(o));
                Ok(())
            }
        }))
        .unwrap();
}

fn inputs(txs: &[Transaction]) -> HashSet<OutPoint> {
    txs.iter()
        .flat_map(|t| t.input.iter().map(|i| i.previous_output))
        .collect()
}

/// The derivation path of the wallet address `script` pays.
fn path_of(f: &Fixture, script: &dashcore::ScriptBuf) -> key_wallet::DerivationPath {
    let session = Arc::clone(&f.session);
    let (id, network) = (f.wallet, dashcore::Network::Regtest);
    let script = script.clone();
    f.engine
        .block_on(session.on_runtime({
            let session = Arc::clone(&session);
            async move {
                let wallet = session.wallet(&id).await?;
                let state = wallet.state().await;
                let address = dashcore::Address::from_script(&script, network).unwrap();
                Ok((state.core_wallet.all_managed_accounts().into_iter())
                    .find_map(|a| a.get_address_info(&address))
                    .expect("an address of the wallet")
                    .path)
            }
        }))
        .unwrap()
}

/// Q4: a move and its consolidation wait for mixing to stop, plans too;
/// stopping lets them run.
#[test]
fn a_move_is_refused_while_the_wallet_mixes() {
    let f = fixture(false);
    f.credit_coinjoin(3, COIN);
    hook(&f, 500, &[]);
    let s = &f.session;
    s.coinjoin.set_mixing_for_test(f.wallet, true);
    let mixing = |r: Result<(), EngineError>| {
        assert!(
            matches!(r, Err(EngineError::CoinJoin(CoinJoinFailure::MixingActive))),
            "{r:?}"
        );
    };
    let plan = || {
        f.engine
            .block_on(s.mixed_coins_sweep_plan(f.wallet, SweepDestination::Wallet))
    };
    mixing(plan().map(|_| ()));
    mixing(
        f.engine
            .block_on(s.mixed_coins_consolidation_plan(f.wallet))
            .map(|_| ()),
    );
    mixing(move_to_wallet(&f).map(|_| ()));
    mixing(consolidate(&f).map(|_| ()));

    s.coinjoin.set_mixing_for_test(f.wallet, false);
    assert_eq!(plan().unwrap().total, COIN);
    assert_eq!(move_to_wallet(&f).unwrap().txids.len(), 1);
}

/// A pause point of the hooks, set by `set` on the installed hooks.
fn pause_at(f: &Fixture, set: impl FnOnce(&mut Hooks, Arc<TestPause>)) -> Arc<TestPause> {
    let pause = Arc::new(TestPause::default());
    set(
        &mut f.session.coinjoin.sweep_hooks.lock().unwrap(),
        Arc::clone(&pause),
    );
    pause
}

/// Releases its pauses when dropped, so a failing assertion leaves no task
/// waiting on one for the fixture's shutdown to wait for.
struct ReleaseOnDrop(Vec<Arc<TestPause>>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        for p in &self.0 {
            p.release.notify_one();
        }
    }
}

/// `fut` on the engine's runtime, or `Internal` after 20 s: a call that
/// waits on a fence another task holds fails the test instead of hanging
/// it.
fn within<T>(
    f: &Fixture,
    fut: impl std::future::Future<Output = Result<T, EngineError>>,
) -> Result<T, EngineError> {
    f.engine.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(20), fut)
            .await
            .unwrap_or_else(|_| Err(EngineError::Internal("waited 20 s".into())))
    })
}

/// Waits (on the test thread) until `pause` is reached.
fn reached(pause: &TestPause) {
    for _ in 0..2_000 {
        if pause.reached.load(Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("pause never reached");
}

/// The move, on the session's runtime, while the test thread goes on.
fn spawn_move(f: &Fixture) -> tokio::task::JoinHandle<Result<SweepResult, EngineError>> {
    let grant = spend_grant(f);
    let (s, w) = (Arc::clone(&f.session), f.wallet);
    (f.session.rt)
        .spawn(async move { s.move_mixed_coins(w, SweepDestination::Wallet, grant).await })
}

fn start(f: &Fixture) -> Result<(), EngineError> {
    within(f, f.session.start_mixing(f.wallet))
}

fn move_running(r: Result<(), EngineError>) {
    assert!(
        matches!(r, Err(EngineError::CoinJoin(CoinJoinFailure::MoveRunning))),
        "{r:?}"
    );
}

/// Review A r1 (Sol R2, Opus m7): mixing and a move claim the CoinJoin
/// account under one lock, the move through its last broadcast. Mixing
/// does not start while a move holds its coins read under the fence, nor
/// while a multi-chunk broadcast waits between chunks; a start that read
/// its view before the move claimed the account is refused when it would
/// set the flag; a second move waits for the first.
#[test]
fn mixing_does_not_start_while_a_move_runs() {
    let f = fixture(false);
    for n in 1..=4 {
        f.credit_coinjoin(n, COIN);
    }

    // The move read its coins under the fence and waits there.
    hook(&f, 500, &[]);
    let read = pause_at(&f, |h, p| h.after_read = Some(p));
    let _release = ReleaseOnDrop(vec![Arc::clone(&read)]);
    let moving = spawn_move(&f);
    reached(&read);
    move_running(start(&f));
    let grant = spend_grant(&f);
    let again = f
        .session
        .move_mixed_coins(f.wallet, SweepDestination::Wallet, grant);
    move_running(within(&f, again).map(drop));
    read.release.notify_one();
    let done = f.engine.block_on(moving).unwrap().unwrap();
    assert_eq!((done.txids.len(), done.failure_code), (1, None));
    assert!(!f.session.coinjoin.is_mixing(&f.wallet));

    // A start read its view, then the move claimed the account: the start
    // is refused when it would set the flag.
    let f = fixture(false);
    for n in 1..=4 {
        f.credit_coinjoin(n, COIN);
    }
    hook(&f, 500, &[]);
    let start_read = pause_at(&f, |h, p| h.start_after_read = Some(p));
    let starting = {
        let (s, w) = (Arc::clone(&f.session), f.wallet);
        (f.session.rt).spawn(async move { s.start_mixing(w).await })
    };
    reached(&start_read);
    let read = pause_at(&f, |h, p| h.after_read = Some(p));
    let _release = ReleaseOnDrop(vec![Arc::clone(&start_read), Arc::clone(&read)]);
    let moving = spawn_move(&f);
    reached(&read);
    start_read.release.notify_one();
    move_running(within(&f, async { starting.await.unwrap() }));
    read.release.notify_one();
    assert_eq!(
        f.engine.block_on(moving).unwrap().unwrap().failure_code,
        None
    );
    assert!(!f.session.coinjoin.is_mixing(&f.wallet));

    // Between the chunks of a multi-chunk broadcast.
    let f = fixture(false);
    for n in 1..=4 {
        f.credit_coinjoin(n, COIN);
    }
    let wire = hook(&f, 2, &[]);
    let between = pause_at(&f, |h, p| h.before_broadcast = Some((1, p)));
    let _release = ReleaseOnDrop(vec![Arc::clone(&between)]);
    let moving = spawn_move(&f);
    reached(&between);
    assert_eq!(wire.txs().len(), 1, "chunk 1 out, chunk 2 waits");
    move_running(start(&f));
    between.release.notify_one();
    let done = f.engine.block_on(moving).unwrap().unwrap();
    assert_eq!((done.txids.len(), done.failure_code), (2, None));
    assert!(!f.session.coinjoin.is_mixing(&f.wallet));
}

/// Q3: the round pays one fresh CoinJoin-account address per chunk, never a
/// BIP44 one, takes every final coin once, in the fewest chunks under the
/// cap, and leaves the coins that are not final.
#[test]
fn a_consolidation_pays_fresh_coinjoin_addresses_and_takes_confirmed_coins_only() {
    let f = fixture(false);
    let coins: Vec<OutPoint> = (1..=6).map(|n| f.credit_coinjoin(n, COIN)).collect();
    let pending = f.credit_coinjoin_with(7, COIN, false);
    let wire = hook(&f, 2, &[]);

    let plan = (f.engine)
        .block_on(f.session.mixed_coins_consolidation_plan(f.wallet))
        .unwrap();
    assert_eq!(
        plan.chunks.iter().map(|c| c.inputs).collect::<Vec<_>>(),
        [2, 2, 2]
    );
    assert_eq!(plan.unconfirmed, 1);
    assert_eq!(plan.coins_after, 4, "three outputs and the pending coin");
    assert_eq!(plan.not_fully_mixed + plan.fully_mixed, 6 * COIN);

    let result = consolidate(&f).unwrap();
    assert_eq!(result.failure_code, None);
    assert_eq!(result.txids.len(), 3);
    let txs = wire.txs();
    assert_eq!(
        inputs(&txs),
        coins.iter().copied().collect(),
        "each confirmed coin once"
    );
    assert!(!inputs(&txs).contains(&pending));
    let mut outputs = HashSet::new();
    for tx in &txs {
        assert_eq!(tx.output.len(), 1, "one output, no change");
        let path = path_of(&f, &tx.output[0].script_pubkey);
        assert!(
            is_coinjoin_path(&path, dashcore::Network::Regtest),
            "{path}"
        );
        assert!(!is_bip44_path(&path, dashcore::Network::Regtest), "{path}");
        assert!(
            outputs.insert(tx.output[0].script_pubkey.clone()),
            "a fresh address each"
        );
    }
}

/// A move to the wallet still pays BIP44, unchanged.
#[test]
fn a_move_to_the_wallet_pays_bip44() {
    let f = fixture(false);
    f.credit_coinjoin(1, COIN);
    let wire = hook(&f, 500, &[]);
    assert_eq!(move_to_wallet(&f).unwrap().txids.len(), 1);
    let path = path_of(&f, &wire.txs()[0].output[0].script_pubkey);
    assert!(is_bip44_path(&path, dashcore::Network::Regtest), "{path}");
}

/// Q3's interrupted round, for both sweeps: chunk 1 out, chunk 2 of unknown
/// outcome, chunk 3 never sent. After a restart (the reservations gone)
/// the next round takes chunk 3's coins only: nothing of chunks 1 and 2 is
/// spent twice, and no extra chunk. Once SPV sees chunk 1, its record goes;
/// chunk 2's inputs stay held until it is seen too, however long that
/// takes.
#[test]
fn an_interrupted_sweep_resumes_without_a_double_spend_or_an_extra_chunk() {
    for consolidation in [true, false] {
        let f = fixture(false);
        for n in 1..=6 {
            f.credit_coinjoin(n, COIN);
        }
        let wire = hook(&f, 2, &[Outcome::Sent, Outcome::Unknown]);
        let run = |f: &Fixture| {
            if consolidation {
                consolidate(f)
            } else {
                move_to_wallet(f)
            }
        };

        let first = run(&f).unwrap();
        assert_eq!(
            first.failure_code.as_deref(),
            Some("coinjoin.broadcast_unknown")
        );
        assert_eq!(first.txids.len(), 1);
        let sent = wire.txs();
        assert_eq!(sent.len(), 2, "chunk 3 was never handed off");
        assert_eq!(first.unknown_txid, Some(sent[1].txid().to_string()));
        forget_claims(&f, &sent);

        let wire = hook(&f, 2, &[]);
        let second = run(&f).unwrap();
        assert_eq!(second.failure_code, None);
        let again = wire.txs();
        assert_eq!(again.len(), 1, "no extra chunk");
        assert!(
            inputs(&again).is_disjoint(&inputs(&sent)),
            "nothing spent twice"
        );
        assert_eq!(inputs(&again).len(), 2, "chunk 3's coins");

        // SPV sees chunk 1 and the second round's chunk: chunk 2's coins are
        // the only ones left, and they stay held.
        see_spent(&f, &[sent[0].clone(), again[0].clone()]);
        forget_claims(&f, &again);
        let held = run(&f);
        assert!(
            matches!(
                held,
                Err(EngineError::CoinJoin(CoinJoinFailure::NothingToMove))
            ),
            "chunk 2 may be out: {held:?}"
        );
        // Seen too: nothing is left to move, and its record went.
        see_spent(&f, &[sent[1].clone()]);
        assert!(matches!(
            run(&f),
            Err(EngineError::CoinJoin(CoinJoinFailure::NothingToMove))
        ));
        assert!(
            sweep_out_keys(&f).is_empty(),
            "every record dropped once seen"
        );
    }
}

/// A definitive rejection drops the record of the rejected chunk and of
/// those after it: their coins go into the next round at once.
#[test]
fn a_rejected_chunk_frees_its_coins_for_the_next_round() {
    let f = fixture(false);
    for n in 1..=6 {
        f.credit_coinjoin(n, COIN);
    }
    let wire = hook(&f, 2, &[Outcome::Sent, Outcome::Rejected]);
    let first = consolidate(&f).unwrap();
    assert_eq!(
        first.failure_code.as_deref(),
        Some("coinjoin.broadcast_rejected")
    );
    assert_eq!(first.unknown_txid, None);
    let sent = wire.txs();
    assert_eq!(sweep_out_keys(&f), HashSet::from([sent[0].txid()]));
    forget_claims(&f, &sent);

    let wire = hook(&f, 2, &[]);
    consolidate(&f).unwrap();
    let again = wire.txs();
    assert_eq!(again.len(), 2, "chunks 2 and 3 again");
    assert!(inputs(&again).is_disjoint(&inputs(&sent[..1])));
}

/// The txids of the wallet's sweep records in `app.sqlite`.
fn sweep_out_keys(f: &Fixture) -> HashSet<Txid> {
    let scope = dw_appdb::local_scope(&f.wallet.to_string());
    f.engine
        .block_on(
            f.session
                .appdb_op(move |db| db.settings_with_prefix(&scope, "coinjoin.sweep_out.")),
        )
        .unwrap()
        .into_iter()
        .map(|(k, _)| k["coinjoin.sweep_out.".len()..].parse().unwrap())
        .collect()
}

/// The consolidation keeps the sweep's gates: a replay refuses it.
#[test]
fn a_consolidation_is_refused_while_a_replay_runs() {
    let f = fixture(false);
    f.credit_coinjoin(3, COIN);
    hook(&f, 500, &[]);
    let s = &f.session;
    let w = f.engine.block_on(s.wallet(&f.wallet)).unwrap();
    let checkpoint = |height: u32| {
        (f.engine).block_on(async { w.state_mut().await.core_wallet.update_synced_height(height) })
    };
    checkpoint(1_000);
    (s.hub).apply_wallet_event(&platform_wallet::events::WalletEvent::SyncHeightAdvanced {
        wallet_id: f.wallet.0,
        height: 1_000,
    });
    checkpoint(500);
    assert!(matches!(
        consolidate(&f),
        Err(EngineError::RescanInProgress)
    ));
}

/// A chunk of unknown outcome holds its coins from every spender after a
/// restart, not only from the next sweep: the coin list reads them
/// reserved. A plan reads its record and keeps it; a sweep drops it once
/// SPV saw the chunk.
#[test]
fn an_unknown_chunk_holds_its_coins_from_every_spender_after_a_restart() {
    let f = fixture(false);
    let coins: Vec<OutPoint> = (1..=4).map(|n| f.credit_coinjoin(n, COIN)).collect();
    let wire = hook(&f, 2, &[Outcome::Unknown]);
    let first = move_to_wallet(&f).unwrap();
    assert_eq!(
        first.failure_code.as_deref(),
        Some("coinjoin.broadcast_unknown")
    );
    let sent = wire.txs();
    assert_eq!(sent.len(), 1);
    forget_claims(&f, &sent);

    let all = crate::coins::CoinFilter {
        include_locked: true,
        ..Default::default()
    };
    let listed = (f.engine).block_on(f.session.utxos(f.wallet, all)).unwrap();
    let held = inputs(&sent);
    assert_eq!(held.len(), 2);
    for o in &coins {
        let coin = listed.iter().find(|c| c.outpoint == *o).unwrap();
        assert_eq!(coin.reserved, held.contains(o), "{o}");
    }

    see_spent(&f, &sent);
    let plan = (f.engine)
        .block_on(
            f.session
                .mixed_coins_sweep_plan(f.wallet, SweepDestination::Wallet),
        )
        .unwrap();
    assert_eq!(plan.total, 2 * COIN);
    assert_eq!(
        sweep_out_keys(&f),
        HashSet::from([sent[0].txid()]),
        "a plan only reads"
    );
    hook(&f, 2, &[]);
    move_to_wallet(&f).unwrap();
    assert!(!sweep_out_keys(&f).contains(&sent[0].txid()));
}

/// The spend grant has to cover the sweep's total: one below it is refused
/// before anything is signed, recorded or reserved.
#[test]
fn a_grant_below_the_total_is_refused_and_nothing_is_sent() {
    let f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    let wire = hook(&f, 500, &[]);
    let grant = f.grant(
        GrantPurpose::Spend {
            max_duffs: 2 * COIN - 1,
        },
        Credential::None,
    );
    let r = f.engine.block_on(f.session.move_mixed_coins(
        f.wallet,
        SweepDestination::Wallet,
        grant,
    ));
    assert!(
        matches!(r, Err(EngineError::CoinJoin(CoinJoinFailure::GrantInvalid))),
        "{r:?}"
    );
    assert!(wire.txs().is_empty());
    assert!(sweep_out_keys(&f).is_empty());
    assert!(f.session.spends.snapshot(&f.wallet).is_empty());
}

/// A rejected consolidation chunk gives its reserved CoinJoin address back:
/// the next round pays the same one, the account's gap does not grow.
#[test]
fn a_rejected_consolidation_gives_its_address_back() {
    let f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    let wire = hook(&f, 500, &[Outcome::Rejected]);
    let first = consolidate(&f).unwrap();
    assert_eq!(
        first.failure_code.as_deref(),
        Some("coinjoin.broadcast_rejected")
    );
    let refused = wire.txs()[0].output[0].script_pubkey.clone();

    let wire = hook(&f, 500, &[]);
    assert_eq!(consolidate(&f).unwrap().failure_code, None);
    assert_eq!(wire.txs()[0].output[0].script_pubkey, refused);
}

/// The wallet's tip (processed and synced heights) moves up to `height`.
fn advance(f: &Fixture, height: u32) {
    let w = f.engine.block_on(f.session.wallet(&f.wallet)).unwrap();
    (f.engine).block_on(async {
        let mut state = w.state_mut().await;
        state.core_wallet.update_last_processed_height(height);
        state.core_wallet.update_synced_height(height);
    });
    (f.session.hub).apply_wallet_event(&platform_wallet::events::WalletEvent::SyncHeightAdvanced {
        wallet_id: f.wallet.0,
        height,
    });
}

/// Q1 (Sol R1, Opus M1): a chunk of unknown outcome is never released on
/// elapsed height. Recorded at height 0, it still holds its coins at 1_000
/// (the old 576-block expiry long past) after a real restart: the session
/// and its SQLite closed and opened again. The next move takes only the
/// coins of the chunk that was never handed off.
#[test]
fn an_unseen_chunk_stays_held_past_the_old_expiry_across_a_reopen() {
    let mut f = fixture(false);
    let coins: Vec<OutPoint> = (1..=4).map(|n| f.credit_coinjoin(n, COIN)).collect();
    let wire = hook(&f, 2, &[Outcome::Unknown]);
    let first = move_to_wallet(&f).unwrap();
    assert_eq!(
        first.failure_code.as_deref(),
        Some("coinjoin.broadcast_unknown")
    );
    let sent = wire.txs();
    assert_eq!(sent.len(), 1, "chunk 2 was never handed off");

    f.reopen();
    for n in 1..=4 {
        f.credit_coinjoin(n, COIN);
    }
    advance(&f, 1_000);
    assert!(f.session.spends.snapshot(&f.wallet).is_empty(), "a restart");
    let wire = hook(&f, 2, &[]);
    let second = move_to_wallet(&f).unwrap();
    assert_eq!(second.failure_code, None);
    let again = wire.txs();
    assert_eq!(again.len(), 1, "only chunk 2's coins");
    assert!(
        inputs(&again).is_disjoint(&inputs(&sent)),
        "chunk 1's coins stay held"
    );
    let held = inputs(&sent);
    assert_eq!(
        inputs(&again),
        coins
            .iter()
            .copied()
            .filter(|o| !held.contains(o))
            .collect()
    );
    assert!(sweep_out_keys(&f).contains(&sent[0].txid()));
}

/// The wallet's history holds `tx`, in the mempool or (`Some(height)`) a
/// block.
fn record_history(f: &Fixture, tx: &Transaction, height: Option<u32>) {
    let context = match height {
        Some(h) => TransactionContext::InBlock(BlockInfo::new(h, BlockHash::all_zeros(), 0)),
        None => TransactionContext::Mempool,
    };
    (f.session.hub.history).with_wallet(f.wallet, |h| {
        h.txs.insert(tx.txid(), history_entry(tx.clone(), context));
    });
}

fn history_entry(tx: Transaction, context: TransactionContext) -> TxEntry {
    TxEntry {
        tx,
        context,
        instant_locked: false,
        first_seen: None,
        own_inputs: Default::default(),
        abandoned: false,
    }
}

/// A transaction other than any chunk spending `input`.
fn conflict(input: OutPoint) -> Transaction {
    Transaction {
        version: 2,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: input,
            ..Default::default()
        }],
        output: vec![dashcore::TxOut {
            value: COIN / 2,
            script_pubkey: dashcore::ScriptBuf::new(),
        }],
        special_transaction_payload: None,
    }
}

/// A block spends `input` in a transaction of its own: SPV drops the coin
/// and records the spender, mined.
fn mine_conflict(f: &Fixture, input: OutPoint) {
    let tx = conflict(input);
    drop_coins(f, &HashSet::from([input]));
    record_history(f, &tx, Some(900));
}

fn resend(f: &Fixture, txid: Option<Txid>) -> Result<Vec<SweepResend>, EngineError> {
    (f.engine).block_on(f.session.resend_mixed_coins(f.wallet, txid))
}

fn resent(txid: Txid, state: SweepResendState) -> Vec<SweepResend> {
    vec![SweepResend { txid, state }]
}

/// Q1: "Try again" and every reconnect send the chunk's recorded bytes,
/// never a new signature; a refusal of a send again (a peer may have the
/// first) keeps the record and its coins. Only the wallet's sight of the
/// chunk drops it. A txid the wallet records no chunk for is refused.
#[test]
fn a_chunk_is_sent_again_with_its_recorded_bytes_until_the_wallet_sees_it() {
    let f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    let wire = hook(&f, 500, &[Outcome::Unknown]);
    move_to_wallet(&f).unwrap();
    let sent = wire.txs();
    assert_eq!(sent.len(), 1);
    let txid = sent[0].txid();
    forget_claims(&f, &sent);

    let wire = hook(&f, 500, &[Outcome::Rejected, Outcome::Unknown]);
    assert_eq!(
        resend(&f, Some(txid)).unwrap(),
        resent(txid, SweepResendState::Unknown),
        "a refusal of a send again is no verdict"
    );
    assert_eq!(sweep_out_keys(&f), HashSet::from([txid]));
    assert!(
        matches!(
            move_to_wallet(&f),
            Err(EngineError::CoinJoin(CoinJoinFailure::NothingToMove))
        ),
        "the coins stay held"
    );
    // A reconnect (SPV peers 0 -> 1) sends it again, in the background.
    let peer: std::net::SocketAddr = "127.0.0.1:9".parse().unwrap();
    dash_spv::EventHandler::on_network_event(
        &*f.session.hub,
        &dash_spv::network::NetworkEvent::PeerConnected { address: peer },
    );
    for _ in 0..2_000 {
        if wire.txs().len() >= 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let wire_txs = wire.txs();
    assert_eq!(wire_txs.len(), 2, "sent again on the reconnect");
    for tx in &wire_txs {
        assert_eq!(
            dashcore::consensus::encode::serialize(tx),
            dashcore::consensus::encode::serialize(&sent[0]),
            "the recorded bytes, not a new signature"
        );
    }
    assert_eq!(sweep_out_keys(&f), HashSet::from([txid]));

    let wrong = Txid::from_byte_array([7; 32]);
    assert!(matches!(
        resend(&f, Some(wrong)),
        Err(EngineError::InvalidArgument(_))
    ));
    see_spent(&f, &sent);
    let wire = hook(&f, 500, &[]);
    assert_eq!(
        resend(&f, None).unwrap(),
        resent(txid, SweepResendState::Resolved)
    );
    assert!(wire.txs().is_empty(), "a seen chunk is not sent");
    assert!(sweep_out_keys(&f).is_empty());
}

/// Q1: a block spending one of a chunk's inputs in another transaction
/// resolves it: the chunk can never confirm, its other inputs are free. A
/// conflict only in the mempool resolves nothing.
#[test]
fn a_mined_conflict_frees_the_chunks_other_inputs() {
    let f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    let wire = hook(&f, 500, &[Outcome::Unknown]);
    move_to_wallet(&f).unwrap();
    let sent = wire.txs();
    forget_claims(&f, &sent);
    let ins: Vec<OutPoint> = sent[0].input.iter().map(|i| i.previous_output).collect();
    assert_eq!(ins.len(), 2);

    record_history(&f, &conflict(ins[0]), None);
    hook(&f, 500, &[]);
    assert!(
        matches!(
            move_to_wallet(&f),
            Err(EngineError::CoinJoin(CoinJoinFailure::NothingToMove))
        ),
        "an unconfirmed conflict resolves nothing"
    );
    assert_eq!(sweep_out_keys(&f), HashSet::from([sent[0].txid()]));

    mine_conflict(&f, ins[0]);
    let wire = hook(&f, 500, &[]);
    assert_eq!(move_to_wallet(&f).unwrap().failure_code, None);
    assert_eq!(inputs(&wire.txs()), HashSet::from([ins[1]]));
    assert!(!sweep_out_keys(&f).contains(&sent[0].txid()));
}

/// Which sweep records still hold their inputs: those the history neither
/// holds nor conflicts in a block, however old; the rest, and rows that do
/// not decode or whose bytes are not the transaction the key names, are
/// for dropping.
#[test]
fn a_sweep_record_resolves_only_when_seen_or_conflicted_in_a_block() {
    use crate::coinjoin::{SweepOut, held_inputs, split_sweep_outs};
    let op = |n: u8| OutPoint::new(Txid::from_byte_array([n; 32]), 0);
    let chunk = |ins: &[u8]| {
        let mut tx = conflict(op(ins[0]));
        tx.input.extend(ins[1..].iter().map(|i| dashcore::TxIn {
            previous_output: op(*i),
            ..Default::default()
        }));
        SweepOut { tx }
    };
    let row = |o: &SweepOut| (o.key(), o.value());
    let pending = chunk(&[1, 2]);
    let seen = chunk(&[3]);
    let mined_conflict = chunk(&[4, 5]);
    let mempool_conflict = chunk(&[6]);
    let bad = ("coinjoin.sweep_out.nope".to_string(), "00".to_string());
    let renamed = (pending.key(), seen.value());
    let mut txs = std::collections::HashMap::new();
    txs.insert(
        seen.txid(),
        history_entry(seen.tx.clone(), TransactionContext::Mempool),
    );
    let mut other = conflict(op(5));
    other.lock_time = 1;
    txs.insert(
        other.txid(),
        history_entry(
            other,
            TransactionContext::InBlock(BlockInfo::new(9, BlockHash::all_zeros(), 0)),
        ),
    );
    let mut loose = conflict(op(6));
    loose.lock_time = 2;
    txs.insert(
        loose.txid(),
        history_entry(loose, TransactionContext::Mempool),
    );
    let (held, mut gone) = split_sweep_outs(
        vec![
            row(&pending),
            row(&seen),
            row(&mined_conflict),
            row(&mempool_conflict),
            bad.clone(),
        ],
        &txs,
    );
    assert_eq!(held, vec![pending.clone(), mempool_conflict.clone()]);
    assert_eq!(held_inputs(&held), [1, 2, 6].map(op).into());
    gone.sort();
    let mut want = vec![seen.key(), mined_conflict.key(), bad.0];
    want.sort();
    assert_eq!(gone, want);
    assert_eq!(
        split_sweep_outs(vec![renamed.clone()], &txs).1,
        vec![renamed.0]
    );
}

/// Marks the credited coin `o` InstantSend-locked: final and trusted, but
/// unconfirmed, its parent still in the mempool.
fn instant_lock(f: &Fixture, o: OutPoint) {
    let session = Arc::clone(&f.session);
    let id = f.wallet;
    f.engine
        .block_on(session.on_runtime({
            let session = Arc::clone(&session);
            async move {
                let wallet = session.wallet(&id).await?;
                let mut state = wallet.state_mut().await;
                let account = (state.core_wallet.accounts.coinjoin_accounts)
                    .get_mut(&0)
                    .unwrap();
                let utxo = account.utxos.get_mut(&o).unwrap();
                utxo.is_instantlocked = true;
                utxo.is_trusted = true;
                Ok(())
            }
        }))
        .unwrap();
}

/// Opus M1 (ancestor limits): a chunk spends confirmed coins only. An
/// unconfirmed coin's parents, a DSTX's foreign inputs among them, are not
/// the wallet's to count, so a full chunk with unconfirmed parents could
/// pass Core's ancestor limits (`-limitancestorcount` 25,
/// `-limitancestorsize` 101 kvB) unseen: such coins wait for a
/// confirmation, for a move and a consolidation alike. Here a chunk of at
/// most 10 inputs with 9 confirmed coins and 3 InstantSend-locked ones:
/// one chunk of the 9, the 3 left out.
#[test]
fn a_full_chunk_never_spends_a_coin_with_unconfirmed_parents() {
    for consolidation in [false, true] {
        let f = fixture(false);
        let confirmed: HashSet<OutPoint> = (1..=9).map(|n| f.credit_coinjoin(n, COIN)).collect();
        for n in 10..=12 {
            let o = f.credit_coinjoin_with(n, COIN, false);
            instant_lock(&f, o);
        }
        let wire = hook(&f, 10, &[]);
        let r = if consolidation {
            consolidate(&f)
        } else {
            move_to_wallet(&f)
        }
        .unwrap();
        assert_eq!(r.failure_code, None);
        let txs = wire.txs();
        assert_eq!(txs.len(), 1, "one chunk");
        assert_eq!(inputs(&txs), confirmed, "confirmed coins only");
    }
}

/// Both plans say how many of the account's coins wait for a
/// confirmation: InstantSend-locked or not, they stay out of every chunk.
#[test]
fn the_plans_count_the_coins_that_wait_for_a_confirmation() {
    let f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(4, COIN);
    f.credit_coinjoin_with(2, COIN, false);
    let locked = f.credit_coinjoin_with(3, COIN, false);
    instant_lock(&f, locked);
    hook(&f, 10, &[]);
    let plan = (f.engine)
        .block_on(
            f.session
                .mixed_coins_sweep_plan(f.wallet, SweepDestination::Wallet),
        )
        .unwrap();
    assert_eq!((plan.total, plan.unconfirmed), (2 * COIN, 2));
    let plan = (f.engine)
        .block_on(f.session.mixed_coins_consolidation_plan(f.wallet))
        .unwrap();
    assert_eq!((plan.unconfirmed, plan.coins_after), (2, 3));
}
