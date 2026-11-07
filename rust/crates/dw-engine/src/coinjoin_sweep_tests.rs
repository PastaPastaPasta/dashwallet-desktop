//! "Move mixed coins" and its consolidation (0.2 MIX-SHIELD unit A)
//! against a real session and vault: the mixing refusal, the consolidation
//! round inside the CoinJoin account, and what a restart keeps of a sweep
//! that may be on the network.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use dashcore::bls_sig_utils::BLSSignature;
use dashcore::hashes::Hash;
use dashcore::{ChainLock, OutPoint, Transaction, Txid};
use dw_vault::{Credential, GrantPurpose, is_bip44_path, is_coinjoin_path};
use key_wallet::account::{AccountType, StandardAccountType};
use key_wallet::wallet::balance::WalletCoreBalance;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use platform_wallet::PlatformWalletError as Pw;
use platform_wallet::events::WalletEvent;

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
/// end) and chunks of at most `max_inputs`. A chunk it does not refuse
/// goes into the wallet's history as a mempool transaction, as dash-spv
/// hands every broadcast to the wallet before any peer has it.
fn hook(f: &Fixture, max_inputs: usize, outcomes: &[Outcome]) -> Arc<Wire> {
    let wire = Arc::new(Wire::default());
    let (w, outcomes) = (Arc::clone(&wire), outcomes.to_vec());
    let (session, wallet) = (Arc::downgrade(&f.session), f.wallet);
    let broadcast: crate::coinjoin::sweep_test_hooks::Broadcast = Arc::new(move |tx| {
        let mut seen = w.seen.lock().unwrap();
        let n = seen.len();
        seen.push(tx.clone());
        let outcome = outcomes.get(n).copied().unwrap_or(Outcome::Sent);
        if !matches!(outcome, Outcome::Rejected)
            && let Some(s) = session.upgrade()
        {
            (s.hub.history).with_wallet(wallet, |h| {
                (h.txs.entry(tx.txid()))
                    .or_insert_with(|| history_entry(tx.clone(), TransactionContext::Mempool));
            });
        }
        match outcome {
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
        // Any round of two coins or more is worth it here; the shield
        // lock's real cap has its own test.
        lock_inputs: Some(1),
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

/// The consolidation plan's quote ("" when there is no plan): the round
/// then runs on what the user would have confirmed.
fn consolidation_quote(f: &Fixture) -> String {
    (f.engine)
        .block_on(f.session.mixed_coins_consolidation_plan(f.wallet))
        .map(|p| p.quote_id)
        .unwrap_or_default()
}

/// The move plan's quote ("" when there is no plan).
fn move_quote(f: &Fixture) -> String {
    (f.engine)
        .block_on(
            f.session
                .mixed_coins_sweep_plan(f.wallet, SweepDestination::Wallet),
        )
        .map(|p| p.quote_id)
        .unwrap_or_default()
}

fn consolidate(f: &Fixture) -> Result<SweepResult, EngineError> {
    let quote = consolidation_quote(f);
    consolidate_with(f, quote)
}

fn consolidate_with(f: &Fixture, quote: String) -> Result<SweepResult, EngineError> {
    let grant = spend_grant(f);
    (f.engine).block_on(f.session.consolidate_mixed_coins(f.wallet, quote, grant))
}

fn move_to_wallet(f: &Fixture) -> Result<SweepResult, EngineError> {
    let quote = move_quote(f);
    move_with(f, quote)
}

fn move_with(f: &Fixture, quote: String) -> Result<SweepResult, EngineError> {
    let grant = spend_grant(f);
    (f.engine).block_on(f.session.move_mixed_coins(
        f.wallet,
        SweepDestination::Wallet,
        quote,
        grant,
    ))
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

/// A block holds `txs`: their inputs are no coins of the wallet any more,
/// and the wallet's history holds them, mined.
fn see_spent(f: &Fixture, txs: &[Transaction]) {
    drop_coins(f, &inputs(txs));
    for tx in txs {
        record_history(f, tx, Some(900));
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
    let (quote, grant) = (move_quote(f), spend_grant(f));
    let (s, w) = (Arc::clone(&f.session), f.wallet);
    (f.session.rt).spawn(async move {
        s.move_mixed_coins(w, SweepDestination::Wallet, quote, grant)
            .await
    })
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

/// Mixing and a move claim the CoinJoin
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
    let again =
        (f.session).move_mixed_coins(f.wallet, SweepDestination::Wallet, String::new(), grant);
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
        // Seen too: nothing is left to move; the records go once their
        // block is ChainLocked.
        see_spent(&f, &[sent[1].clone()]);
        assert!(matches!(
            run(&f),
            Err(EngineError::CoinJoin(CoinJoinFailure::NothingToMove))
        ));
        assert_eq!(sweep_out_keys(&f).len(), 3, "their block is not final");
        chainlock(&f, 900);
        assert!(matches!(
            run(&f),
            Err(EngineError::CoinJoin(CoinJoinFailure::NothingToMove))
        ));
        assert!(
            sweep_out_keys(&f).is_empty(),
            "every record dropped once final"
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

/// The txids of the wallet's beaten-chunk rows in `app.sqlite`.
fn beaten_keys(f: &Fixture) -> HashSet<Txid> {
    let scope = dw_appdb::local_scope(&f.wallet.to_string());
    f.engine
        .block_on(
            f.session
                .appdb_op(move |db| db.settings_with_prefix(&scope, "coinjoin.sweep_beaten.")),
        )
        .unwrap()
        .into_iter()
        .map(|(k, _)| k["coinjoin.sweep_beaten.".len()..].parse().unwrap())
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
    assert!(
        sweep_out_keys(&f).contains(&sent[0].txid()),
        "its block is not final"
    );
    chainlock(&f, 900);
    hook(&f, 2, &[]);
    resend(&f, None).unwrap();
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
    let quote = move_quote(&f);
    let r = f.engine.block_on(f.session.move_mixed_coins(
        f.wallet,
        SweepDestination::Wallet,
        quote,
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

/// A chunk of unknown outcome is never released on
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
    // Every spender reads the record, not only the next move: a
    // send of a held coin and mixing's view of it.
    let held_coin = sent[0].input[0].previous_output;
    let send = f.draft_to("yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n".into(), COIN / 2);
    send.set_source(crate::send::CoinSource::Outpoints(vec![held_coin]))
        .unwrap();
    assert!(
        matches!(
            f.engine.block_on(send.estimate()),
            Err(EngineError::Send(crate::send::SendFailure::OutpointUnavailable(o))) if o == held_coin
        ),
        "a send may not take a held coin"
    );
    let session = Arc::clone(&f.session);
    let id = f.wallet;
    let view = (f.engine)
        .block_on(session.on_runtime({
            let s = Arc::clone(&session);
            async move {
                let fence = s.coin_fence(id).await?;
                s.mix_view(&fence).await
            }
        }))
        .unwrap();
    let held = inputs(&sent);
    for c in view
        .coins
        .iter()
        .filter(|c| held.contains(&c.coin.utxo.outpoint))
    {
        assert!(
            c.coin.reserved,
            "mixing reads {} reserved",
            c.coin.utxo.outpoint
        );
    }
    // The stored history brings the unconfirmed chunk back as Mempool.
    record_history(&f, &sent[0], None);
    let wire = hook(&f, 2, &[]);
    assert_eq!(
        resend(&f, None).unwrap(),
        resent(sent[0].txid(), SweepResendState::Sent),
        "sent again after the restart"
    );
    assert_eq!(wire.txs(), sent, "the recorded bytes");
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

/// The wallet's history holds `tx`, in the mempool or (`Some(height)`) the
/// block the chain holds at that height.
fn record_history(f: &Fixture, tx: &Transaction, height: Option<u32>) {
    let context = match height {
        Some(h) => TransactionContext::InBlock(BlockInfo::new(h, f.chain.hash(h), 0)),
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

/// A block spends `input` in a transaction of its own that pays the
/// wallet: SPV drops the coin and records the spender, mined.
fn mine_conflict(f: &Fixture, input: OutPoint) -> Txid {
    let tx = conflict(input);
    drop_coins(f, &HashSet::from([input]));
    record_history(f, &tx, Some(900));
    tx.txid()
}

/// A ChainLock at `height`, as key-wallet reports it: the history's
/// records in blocks at or below it are promoted.
fn chainlock(f: &Fixture, height: u32) {
    let locked: Vec<Txid> = (f.session.hub.history).read(&f.wallet, |txs| {
        (txs.iter())
            .filter(|(_, e)| e.context.block_info().is_some_and(|b| b.height() <= height))
            .map(|(t, _)| *t)
            .collect()
    });
    (f.session.hub).apply_wallet_event(&WalletEvent::ChainLockProcessed {
        wallet_id: f.wallet.0,
        chain_lock: ChainLock {
            block_height: height,
            block_hash: BlockHash::all_zeros(),
            signature: BLSSignature::from([0; 96]),
        },
        locked_transactions: std::collections::BTreeMap::from([(
            AccountType::Standard {
                index: 0,
                standard_account_type: StandardAccountType::BIP44Account,
            },
            locked,
        )]),
    });
}

/// A fork replaces the blocks above `height`: the header chain holds new
/// blocks there, and key-wallet drops `removed`, the transactions they
/// held, and gives back the coins they spent.
fn truncate(f: &Fixture, height: u32, removed: &[Txid]) {
    f.chain.fork_above(height);
    (f.session.hub).apply_wallet_event(&WalletEvent::ChainTruncated {
        wallet_id: f.wallet.0,
        height,
        txids: removed.to_vec(),
        unconfirmed_txids: Vec::new(),
        restored_outpoints: Vec::new(),
        balance: WalletCoreBalance::default(),
        account_balances: std::collections::BTreeMap::new(),
    });
}

/// key-wallet's conflict sweep: `winner`, mined in the block the chain
/// holds at `height` or InstantSend-locked (`None`), beat the recorded
/// `loser` to an input. The winner pays no address of the wallet, so this
/// is the only event that names it.
fn swept(f: &Fixture, loser: Txid, winner: Txid, height: Option<u32>) {
    swept_in(f, loser, winner, height.map(|h| (h, f.chain.hash(h))));
}

/// [`swept`] with the winner's block as key-wallet processed it.
fn swept_in(f: &Fixture, loser: Txid, winner: Txid, block: Option<(u32, BlockHash)>) {
    (f.session.hub).apply_wallet_event(&WalletEvent::TransactionsSwept {
        wallet_id: f.wallet.0,
        txids: vec![loser],
        superseded_by: winner,
        winner_block: block.map(|(h, hash)| BlockInfo::new(h, hash, 0)),
        released_outpoints: Vec::new(),
        balance: WalletCoreBalance::default(),
        account_balances: std::collections::BTreeMap::new(),
    });
    settle(f);
}

/// Waits for the sweep row checks the session started.
fn settle(f: &Fixture) {
    let checks = f.session.hub.take_sweep_checks();
    f.engine.block_on(async {
        for c in checks {
            c.await.unwrap();
        }
    });
}

/// `chunk`'s inputs are out of every spend: a send refuses one, and mixing
/// reads them reserved.
fn assert_held(f: &Fixture, chunk: &Transaction) {
    let held_coin = chunk.input[0].previous_output;
    let send = f.draft_to("yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n".into(), COIN / 2);
    send.set_source(crate::send::CoinSource::Outpoints(vec![held_coin]))
        .unwrap();
    assert!(
        matches!(
            f.engine.block_on(send.estimate()),
            Err(EngineError::Send(crate::send::SendFailure::OutpointUnavailable(o))) if o == held_coin
        ),
        "a send may not take a held coin"
    );
    let session = Arc::clone(&f.session);
    let id = f.wallet;
    let view = (f.engine)
        .block_on(session.on_runtime({
            let s = Arc::clone(&session);
            async move {
                let fence = s.coin_fence(id).await?;
                s.mix_view(&fence).await
            }
        }))
        .unwrap();
    let held = inputs(std::slice::from_ref(chunk));
    let seen: Vec<_> = (view.coins.iter())
        .filter(|c| held.contains(&c.coin.utxo.outpoint))
        .collect();
    assert_eq!(seen.len(), held.len(), "mixing lists every input");
    for c in seen {
        assert!(
            c.coin.reserved,
            "mixing reads {} reserved",
            c.coin.utxo.outpoint
        );
    }
}

fn resend(f: &Fixture, txid: Option<Txid>) -> Result<Vec<SweepResend>, EngineError> {
    (f.engine).block_on(f.session.resend_mixed_coins(f.wallet, txid))
}

fn resent(txid: Txid, state: SweepResendState) -> Vec<SweepResend> {
    vec![SweepResend { txid, state }]
}

/// Q1: "Try again" and every reconnect send the chunk's recorded bytes,
/// never a new signature; a refusal of a send again (a peer may have the
/// first) keeps the record and its coins. The wallet's own mempool record
/// of the chunk is no sight of it; only a block (or an InstantSend lock)
/// drops it. A txid the wallet no longer records a chunk for reads
/// resolved.
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
    assert!(
        (f.session.hub.history).read(&f.wallet, |txs| txs.contains_key(&txid)),
        "the wallet records its broadcast at once"
    );

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
    assert_eq!(
        resend(&f, Some(wrong)).unwrap(),
        resent(wrong, SweepResendState::Resolved)
    );
    see_spent(&f, &sent);
    let wire = hook(&f, 500, &[]);
    assert_eq!(
        resend(&f, None).unwrap(),
        resent(txid, SweepResendState::Resolved)
    );
    assert!(wire.txs().is_empty(), "a seen chunk is not sent");
    assert_eq!(
        sweep_out_keys(&f),
        HashSet::from([txid]),
        "its block is not final"
    );
    chainlock(&f, 900);
    assert_eq!(
        resend(&f, None).unwrap(),
        resent(txid, SweepResendState::Resolved)
    );
    assert!(sweep_out_keys(&f).is_empty(), "its block is final");
}

/// A transaction that pays no address of the wallet, mined, beats a
/// chunk to one of its inputs: key-wallet never records it, and names it
/// only in its conflict sweep. The chunk can never confirm on this chain:
/// its other inputs are free. A conflict only in the mempool resolves
/// nothing, and the record goes once the winner's block is ChainLocked.
#[test]
fn a_conflict_that_pays_no_wallet_address_frees_the_chunks_other_inputs() {
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

    let mut winner = conflict(ins[0]);
    winner.lock_time = 1;
    let winner = winner.txid();
    drop_coins(&f, &HashSet::from([ins[0]]));
    swept(&f, sent[0].txid(), winner, Some(900));
    let wire = hook(&f, 500, &[]);
    assert_eq!(move_to_wallet(&f).unwrap().failure_code, None);
    assert_eq!(inputs(&wire.txs()), HashSet::from([ins[1]]));
    assert!(
        sweep_out_keys(&f).contains(&sent[0].txid()),
        "the winner's block is not final"
    );

    chainlock(&f, 900);
    resend(&f, None).unwrap();
    assert!(!sweep_out_keys(&f).contains(&sent[0].txid()));
    assert!(beaten_keys(&f).is_empty());
}

/// An InstantSend-locked winner is final at once: the record goes.
#[test]
fn a_chunk_an_instant_send_locked_transaction_beat_is_dropped() {
    let f = fixture(false);
    f.credit_coinjoin(1, COIN);
    let wire = hook(&f, 500, &[Outcome::Unknown]);
    move_to_wallet(&f).unwrap();
    let sent = wire.txs();
    forget_claims(&f, &sent);
    let input = sent[0].input[0].previous_output;
    swept(&f, sent[0].txid(), conflict(input).txid(), None);
    assert_eq!(
        resend(&f, Some(sent[0].txid())).unwrap(),
        resent(sent[0].txid(), SweepResendState::Resolved)
    );
    assert!(sweep_out_keys(&f).is_empty());
    assert!(beaten_keys(&f).is_empty());
}

/// How a chunk's resolution is pruned: the step that prunes it.
#[derive(Clone, Copy, Debug)]
enum Prune {
    Resend,
    Move,
}

/// A chunk of unknown outcome is mined in a block that is not final, and a
/// resend or the next move reads it resolved. A fork then replaces that
/// block: after a restart (session and SQLite closed and opened again) the
/// chunk is pending again. Its inputs are out of every spend and the
/// resend sends its recorded bytes.
#[test]
fn a_chunk_whose_block_a_fork_replaces_is_pending_again() {
    for prune in [Prune::Resend, Prune::Move] {
        let mut f = fixture(false);
        for n in 1..=4 {
            f.credit_coinjoin(n, COIN);
        }
        let wire = hook(&f, 2, &[Outcome::Unknown]);
        move_to_wallet(&f).unwrap();
        let sent = wire.txs();
        assert_eq!(sent.len(), 1, "{prune:?}");
        let txid = sent[0].txid();
        forget_claims(&f, &sent);

        see_spent(&f, &sent);
        let wire = hook(&f, 2, &[]);
        match prune {
            Prune::Resend => assert_eq!(
                resend(&f, None).unwrap(),
                resent(txid, SweepResendState::Resolved)
            ),
            Prune::Move => {
                assert_eq!(move_to_wallet(&f).unwrap().failure_code, None);
                assert!(inputs(&wire.txs()).is_disjoint(&inputs(&sent)));
            }
        }
        truncate(&f, 899, &[txid]);

        f.reopen();
        for n in 1..=4 {
            f.credit_coinjoin(n, COIN);
        }
        assert_held(&f, &sent[0]);
        let wire = hook(&f, 2, &[]);
        assert_eq!(
            resend(&f, Some(txid)).unwrap(),
            resent(txid, SweepResendState::Sent),
            "{prune:?}"
        );
        assert_eq!(wire.txs(), sent, "the recorded bytes");
        let wire = hook(&f, 2, &[]);
        let _ = move_to_wallet(&f);
        assert!(
            inputs(&wire.txs()).is_disjoint(&inputs(&sent)),
            "a move may not take them ({prune:?})"
        );
    }
}

/// The same when a transaction of the wallet's that spends one of the
/// chunk's inputs is mined in a block a fork then replaces.
#[test]
fn a_chunk_whose_conflict_a_fork_replaces_is_pending_again() {
    let mut f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    let wire = hook(&f, 500, &[Outcome::Unknown]);
    move_to_wallet(&f).unwrap();
    let sent = wire.txs();
    let txid = sent[0].txid();
    forget_claims(&f, &sent);
    let rival = mine_conflict(&f, sent[0].input[0].previous_output);
    assert_eq!(
        resend(&f, None).unwrap(),
        resent(txid, SweepResendState::Resolved)
    );
    truncate(&f, 899, &[rival]);

    f.reopen();
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    assert_held(&f, &sent[0]);
    let wire = hook(&f, 500, &[]);
    assert_eq!(
        resend(&f, None).unwrap(),
        resent(txid, SweepResendState::Sent)
    );
    assert_eq!(wire.txs(), sent, "the recorded bytes");
}

/// A winner that pays no address of the wallet, in a block a fork then
/// replaces: the truncation clears what the conflict sweep recorded, and
/// after a restart the chunk is pending again.
#[test]
fn a_chunk_whose_unrecorded_winner_a_fork_replaces_is_pending_again() {
    let mut f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    let wire = hook(&f, 500, &[Outcome::Unknown]);
    move_to_wallet(&f).unwrap();
    let sent = wire.txs();
    let txid = sent[0].txid();
    forget_claims(&f, &sent);
    let input = sent[0].input[0].previous_output;
    drop_coins(&f, &HashSet::from([input]));
    swept(&f, txid, conflict(input).txid(), Some(900));
    assert_eq!(
        resend(&f, None).unwrap(),
        resent(txid, SweepResendState::Resolved)
    );
    assert_eq!(beaten_keys(&f), HashSet::from([txid]));
    truncate(&f, 899, &[]);
    assert!(beaten_keys(&f).is_empty());

    f.reopen();
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    assert_held(&f, &sent[0]);
    let wire = hook(&f, 500, &[]);
    assert_eq!(
        resend(&f, None).unwrap(),
        resent(txid, SweepResendState::Sent)
    );
    assert_eq!(wire.txs(), sent, "the recorded bytes");
}

/// The wallet's store records `tx` mined in the block the chain holds at
/// `height`, as SPV's persister does, and the session's history sees it.
fn store_mined(f: &Fixture, tx: &Transaction, height: u32) {
    use key_wallet::managed_account::transaction_record::{
        TransactionDirection, TransactionRecord,
    };
    use key_wallet::transaction_checking::transaction_router::TransactionType;
    let record = TransactionRecord::new(
        tx.clone(),
        AccountType::Standard {
            index: 0,
            standard_account_type: StandardAccountType::BIP44Account,
        },
        TransactionContext::InBlock(BlockInfo::new(height, f.chain.hash(height), 0)),
        TransactionType::Standard,
        TransactionDirection::Internal,
        Vec::new(),
        Vec::new(),
        0,
    );
    store_core(
        f,
        platform_wallet::changeset::CoreChangeSet {
            records: vec![record.clone()],
            ..Default::default()
        },
    );
    (f.session.hub.history).with_wallet(f.wallet, |h| {
        h.upsert(&record, None);
    });
}

/// Writes `core` to the wallet's store, durably.
fn store_core(f: &Fixture, core: platform_wallet::changeset::CoreChangeSet) {
    use platform_wallet::changeset::{PlatformWalletChangeSet, PlatformWalletPersistence};
    let live = f.session.live().unwrap();
    live.persister
        .store(
            f.wallet.0,
            PlatformWalletChangeSet {
                core: Some(core),
                ..Default::default()
            },
        )
        .unwrap();
    live.persister.flush(f.wallet.0).unwrap();
}

/// Whether `app.sqlite` refuses to delete a beaten row (a storage failure
/// on the truncation's write).
fn refuse_beaten_deletes(f: &Fixture, refuse: bool) {
    let db = rusqlite::Connection::open(f.session.data_dir().join(dw_appdb::APP_DB_FILE)).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    if refuse {
        db.execute_batch(
            "CREATE TRIGGER refuse_beaten_delete BEFORE DELETE ON settings_kv
             WHEN old.key LIKE 'coinjoin.sweep_beaten.%'
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .unwrap();
    } else {
        db.execute_batch("DROP TRIGGER refuse_beaten_delete;")
            .unwrap();
    }
}

/// Whether mixing's view reads `o` reserved.
fn mix_reserved(f: &Fixture, o: OutPoint) -> bool {
    let session = Arc::clone(&f.session);
    let id = f.wallet;
    let view = (f.engine)
        .block_on(session.on_runtime({
            let s = Arc::clone(&session);
            async move {
                let fence = s.coin_fence(id).await?;
                s.mix_view(&fence).await
            }
        }))
        .unwrap();
    (view.coins.iter())
        .find(|c| c.coin.utxo.outpoint == o)
        .expect("a coin of the wallet")
        .coin
        .reserved
}

/// The chunk is mined in a block the wallet's store records, and a fork
/// replaces that block. The pinned persister keeps the record (it mirrors
/// no truncation), so a restart reloads it in its old block: no evidence,
/// as the chain holds another block there. The chunk is pending again, its
/// inputs held and its bytes sent again, also once the replacement chain's
/// ChainLock passes that height (which promotes the stale record).
#[test]
fn a_resolution_a_fork_replaced_is_no_evidence_after_a_restart() {
    let mut f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    let wire = hook(&f, 500, &[Outcome::Unknown]);
    move_to_wallet(&f).unwrap();
    let sent = wire.txs();
    let txid = sent[0].txid();
    drop_coins(&f, &inputs(&sent));
    store_mined(&f, &sent[0], 900);
    assert_eq!(
        resend(&f, None).unwrap(),
        resent(txid, SweepResendState::Resolved)
    );
    truncate(&f, 899, &[txid]);

    f.reopen();
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    let reloaded = (f.session.hub.history).read(&f.wallet, |txs| {
        txs.get(&txid)
            .and_then(|e| e.context.block_info())
            .map(|b| b.height())
    });
    assert_eq!(reloaded, Some(900), "the store keeps the replaced branch");
    for chainlocked in [false, true] {
        if chainlocked {
            chainlock(&f, 901);
        }
        assert_held(&f, &sent[0]);
        let wire = hook(&f, 500, &[]);
        assert_eq!(
            resend(&f, Some(txid)).unwrap(),
            resent(txid, SweepResendState::Sent),
            "chainlocked={chainlocked}"
        );
        assert_eq!(wire.txs(), sent, "the recorded bytes");
        assert!(sweep_out_keys(&f).contains(&txid), "no prune");
    }
}

/// How the sweep rows recover from a truncation whose write failed.
#[derive(Debug, Clone, Copy)]
enum Recover {
    /// The wallet's next event checks them again.
    NextEvent,
    /// A restart: the next read checks them against the header chain.
    Restart,
}

/// A winner that pays no address of the wallet beats the chunk in a block
/// a fork then replaces, and `app.sqlite` refuses the truncation's delete
/// of the beaten row. The row names a block the chain no longer holds, so
/// it is no evidence: the chunk is pending at once, its inputs held after
/// the storage recovers and the session restarts, its bytes sent again,
/// and the replacement chain's ChainLock finalizes nothing. The row is
/// deleted on the wallet's next event, or by the first read after a
/// restart.
#[test]
fn a_beaten_row_a_failed_truncation_left_is_no_evidence() {
    for recover in [Recover::NextEvent, Recover::Restart] {
        let mut f = fixture(false);
        f.credit_coinjoin(1, COIN);
        f.credit_coinjoin(2, COIN);
        let wire = hook(&f, 500, &[Outcome::Unknown]);
        move_to_wallet(&f).unwrap();
        let sent = wire.txs();
        let txid = sent[0].txid();
        forget_claims(&f, &sent);
        let input = sent[0].input[0].previous_output;
        drop_coins(&f, &HashSet::from([input]));
        swept(&f, txid, conflict(input).txid(), Some(900));
        assert_eq!(
            resend(&f, None).unwrap(),
            resent(txid, SweepResendState::Resolved)
        );

        refuse_beaten_deletes(&f, true);
        truncate(&f, 899, &[]);
        assert_eq!(
            beaten_keys(&f),
            HashSet::from([txid]),
            "the delete was refused"
        );
        let wire = hook(&f, 500, &[]);
        assert_eq!(
            resend(&f, None).unwrap(),
            resent(txid, SweepResendState::Sent),
            "{recover:?}: pending at once"
        );
        assert_eq!(wire.txs(), sent);

        refuse_beaten_deletes(&f, false);
        if let Recover::NextEvent = recover {
            (f.session.hub).apply_wallet_event(&WalletEvent::SyncHeightAdvanced {
                wallet_id: f.wallet.0,
                height: 902,
            });
            settle(&f);
            assert!(beaten_keys(&f).is_empty(), "the next event deleted it");
        }
        f.reopen();
        f.credit_coinjoin(1, COIN);
        f.credit_coinjoin(2, COIN);
        chainlock(&f, 901);
        assert_held(&f, &sent[0]);
        let wire = hook(&f, 500, &[]);
        assert_eq!(
            resend(&f, None).unwrap(),
            resent(txid, SweepResendState::Sent),
            "{recover:?}"
        );
        assert_eq!(wire.txs(), sent, "the recorded bytes");
        assert!(beaten_keys(&f).is_empty(), "{recover:?}: deleted");
        assert!(sweep_out_keys(&f).contains(&txid), "{recover:?}: no prune");
    }
}

/// A winner that pays no address of the wallet takes one input of a chunk
/// whose broadcast outcome is unknown. In the same session, with the
/// sweep's own reservations kept, the input the winner left is free once
/// the chunk is beaten, and again once that is final.
#[test]
fn a_beaten_chunk_frees_the_input_its_winner_left_in_the_same_session() {
    let f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    let wire = hook(&f, 500, &[Outcome::Unknown]);
    move_to_wallet(&f).unwrap();
    let sent = wire.txs();
    let txid = sent[0].txid();
    let ins: Vec<OutPoint> = sent[0].input.iter().map(|i| i.previous_output).collect();
    assert!(mix_reserved(&f, ins[1]), "the sweep holds it");
    drop_coins(&f, &HashSet::from([ins[0]]));
    swept(&f, txid, conflict(ins[0]).txid(), Some(900));
    assert!(!mix_reserved(&f, ins[1]), "beaten: the other input is free");
    chainlock(&f, 900);
    assert!(!mix_reserved(&f, ins[1]), "final: still free");
    let wire = hook(&f, 500, &[]);
    assert_eq!(move_to_wallet(&f).unwrap().failure_code, None);
    assert_eq!(inputs(&wire.txs()), HashSet::from([ins[1]]));
    assert!(!sweep_out_keys(&f).contains(&txid));
}

/// A record of a chunk in a block below the ChainLock that key-wallet
/// stored: after a restart, the stored ChainLock alone drops it, with no
/// ChainLock event.
#[test]
fn a_chainlock_loaded_at_a_restart_drops_a_record_on_its_own() {
    let mut f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    let wire = hook(&f, 500, &[Outcome::Unknown]);
    move_to_wallet(&f).unwrap();
    let sent = wire.txs();
    let txid = sent[0].txid();
    drop_coins(&f, &inputs(&sent));
    store_mined(&f, &sent[0], 900);
    store_core(
        &f,
        platform_wallet::changeset::CoreChangeSet {
            last_applied_chain_lock: Some(ChainLock {
                block_height: 900,
                block_hash: f.chain.hash(900),
                signature: BLSSignature::from([0; 96]),
            }),
            ..Default::default()
        },
    );
    assert_eq!(
        resend(&f, None).unwrap(),
        resent(txid, SweepResendState::Resolved)
    );
    assert!(sweep_out_keys(&f).contains(&txid), "no ChainLock seen yet");

    f.reopen();
    assert_eq!(f.session.hub.best_chainlock().map(|(h, _)| h), Some(900));
    assert_eq!(
        resend(&f, None).unwrap(),
        resent(txid, SweepResendState::Resolved)
    );
    assert!(
        sweep_out_keys(&f).is_empty(),
        "the stored ChainLock dropped it"
    );
}

/// A conflict of the wallet's in a block a fork replaces, then mined again
/// in the new branch: resolved again, and final under that branch's
/// ChainLock.
#[test]
fn a_conflict_mined_again_after_a_fork_resolves_the_chunk_again() {
    let f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    let wire = hook(&f, 500, &[Outcome::Unknown]);
    move_to_wallet(&f).unwrap();
    let sent = wire.txs();
    let txid = sent[0].txid();
    forget_claims(&f, &sent);
    let ins: Vec<OutPoint> = sent[0].input.iter().map(|i| i.previous_output).collect();
    let rival = conflict(ins[0]);
    let rival_id = mine_conflict(&f, ins[0]);
    assert_eq!(
        resend(&f, None).unwrap(),
        resent(txid, SweepResendState::Resolved)
    );
    truncate(&f, 899, &[rival_id]);
    assert!(mix_reserved(&f, ins[1]), "pending again");

    record_history(&f, &rival, Some(901));
    assert_eq!(
        resend(&f, None).unwrap(),
        resent(txid, SweepResendState::Resolved)
    );
    assert!(!mix_reserved(&f, ins[1]), "resolved again");
    chainlock(&f, 901);
    resend(&f, None).unwrap();
    assert!(
        !sweep_out_keys(&f).contains(&txid),
        "final on the new branch"
    );
}

/// A winner that pays no address of the wallet beats the chunk, a fork
/// replaces its block, and the new branch mines the same winner again. No
/// event names it (it pays no wallet address and no recorded transaction
/// spends the input), so nothing is evidence: the chunk stays pending
/// (E0-04), held and sent again, past the new branch's ChainLock and a
/// restart.
#[test]
fn a_winner_mined_again_after_a_fork_leaves_its_chunk_pending() {
    let mut f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    let wire = hook(&f, 500, &[Outcome::Unknown]);
    move_to_wallet(&f).unwrap();
    let sent = wire.txs();
    let txid = sent[0].txid();
    forget_claims(&f, &sent);
    let input = sent[0].input[0].previous_output;
    drop_coins(&f, &HashSet::from([input]));
    swept(&f, txid, conflict(input).txid(), Some(900));
    truncate(&f, 899, &[]);
    chainlock(&f, 905);
    for restarted in [false, true] {
        if restarted {
            f.reopen();
            f.credit_coinjoin(1, COIN);
            f.credit_coinjoin(2, COIN);
            assert_held(&f, &sent[0]);
        }
        let wire = hook(&f, 500, &[]);
        assert_eq!(
            resend(&f, None).unwrap(),
            resent(txid, SweepResendState::Sent),
            "restarted={restarted}"
        );
        assert_eq!(wire.txs(), sent);
        assert!(sweep_out_keys(&f).contains(&txid));
    }

    // The heavy way out today: removing the wallet deletes its app rows,
    // the sweep records with them, and an import from the seed starts
    // without them.
    remove_and_import_again(&f);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    assert!(
        sweep_out_keys(&f).is_empty(),
        "the removal dropped the record"
    );
    assert!(
        !mix_reserved(&f, sent[0].input[1].previous_output),
        "its other input is free"
    );
}

/// A winner that pays no address of the wallet beats the chunk in block A
/// at 900, and the header chain moves to a branch with another block there
/// before the wallet's event reaches the session, with no truncation
/// delivered (dash-spv stops its wallet-event monitor while it truncates).
/// The event names A, so its row is no evidence on the new branch: the
/// chunk stays pending, held and sent again, past that branch's ChainLock
/// and a restart.
#[test]
fn a_winner_whose_block_a_fork_replaced_before_its_event_is_no_evidence() {
    let mut f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    let wire = hook(&f, 500, &[Outcome::Unknown]);
    move_to_wallet(&f).unwrap();
    let sent = wire.txs();
    let txid = sent[0].txid();
    forget_claims(&f, &sent);
    let input = sent[0].input[0].previous_output;
    drop_coins(&f, &HashSet::from([input]));
    let mined_in = f.chain.hash(900);
    f.chain.fork_above(899);
    swept_in(&f, txid, conflict(input).txid(), Some((900, mined_in)));
    chainlock(&f, 905);
    for restarted in [false, true] {
        if restarted {
            f.reopen();
            f.credit_coinjoin(1, COIN);
            f.credit_coinjoin(2, COIN);
            chainlock(&f, 905);
            assert_held(&f, &sent[0]);
        } else {
            assert!(
                mix_reserved(&f, sent[0].input[1].previous_output),
                "the other input is held"
            );
        }
        let wire = hook(&f, 500, &[]);
        assert_eq!(
            resend(&f, None).unwrap(),
            resent(txid, SweepResendState::Sent),
            "restarted={restarted}"
        );
        assert_eq!(wire.txs(), sent, "the recorded bytes");
        assert!(sweep_out_keys(&f).contains(&txid), "no prune");
    }
}

/// A beaten row in the older `height winner` form names no block. While
/// the header store has no header at its height it is no evidence, and
/// kept; once it has one, the row does not match it and is dropped. The
/// chunk is pending throughout.
#[test]
fn a_legacy_beaten_row_is_no_evidence_and_goes_once_a_header_is_known() {
    let f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    let wire = hook(&f, 500, &[Outcome::Unknown]);
    move_to_wallet(&f).unwrap();
    let sent = wire.txs();
    let txid = sent[0].txid();
    forget_claims(&f, &sent);
    let input = sent[0].input[0].previous_output;
    drop_coins(&f, &HashSet::from([input]));
    let scope = dw_appdb::local_scope(&f.wallet.to_string());
    let (key, value) = (
        format!("coinjoin.sweep_beaten.{txid}"),
        format!("900 {}", conflict(input).txid()),
    );
    f.chain.set_known(900, false);
    (f.engine)
        .block_on(f.session.appdb_op(move |db| {
            db.set_settings_durable(&scope, &[(key.as_str(), Some(value.as_str()))])
        }))
        .unwrap();
    chainlock(&f, 905);
    for known in [false, true] {
        f.chain.set_known(900, known);
        let wire = hook(&f, 500, &[]);
        assert_eq!(
            resend(&f, None).unwrap(),
            resent(txid, SweepResendState::Sent),
            "known={known}"
        );
        assert_eq!(wire.txs(), sent, "the recorded bytes");
        assert!(
            mix_reserved(&f, sent[0].input[1].previous_output),
            "known={known}: the other input is held"
        );
        assert_eq!(beaten_keys(&f).contains(&txid), !known, "known={known}");
        assert!(
            sweep_out_keys(&f).contains(&txid),
            "known={known}: no prune"
        );
    }
}

/// Removes the wallet (a wipe grant) and imports it again from its seed:
/// the same wallet id.
fn remove_and_import_again(f: &Fixture) {
    let wipe = (f.session.vault())
        .authorize(GrantPurpose::Wipe, Some(&f.wallet.0), Credential::None)
        .unwrap()
        .id;
    (f.engine)
        .block_on(f.session.remove_wallet(f.wallet, wipe))
        .unwrap();
    let again = (f.engine)
        .block_on(
            f.session.import_wallet(
                zeroize::Zeroizing::new(
                    crate::platform::lease::session_tests::PHRASE
                        .as_bytes()
                        .to_vec(),
                ),
                zeroize::Zeroizing::new(Vec::new()),
                crate::ImportOptions {
                    birth_height: Some(0),
                    core_compat: false,
                    ..Default::default()
                },
            ),
        )
        .unwrap();
    assert_eq!(again, f.wallet);
}

/// A final resolution clears the record: the chunk in a block that is then
/// ChainLocked, or InstantSend-locked; a conflict of the wallet's in a
/// ChainLocked block frees the chunk's other inputs.
#[test]
fn a_final_resolution_drops_the_record() {
    let f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    f.credit_coinjoin(3, COIN);
    let wire = hook(&f, 1, &[Outcome::Unknown]);
    move_to_wallet(&f).unwrap();
    let mined = wire.txs();
    forget_claims(&f, &mined);
    see_spent(&f, &mined);
    chainlock(&f, 900);
    assert_eq!(
        resend(&f, None).unwrap(),
        resent(mined[0].txid(), SweepResendState::Resolved)
    );
    assert!(sweep_out_keys(&f).is_empty(), "ChainLocked");

    let wire = hook(&f, 1, &[Outcome::Unknown]);
    move_to_wallet(&f).unwrap();
    let locked = wire.txs();
    forget_claims(&f, &locked);
    (f.session.hub.history).with_wallet(f.wallet, |h| {
        h.txs.get_mut(&locked[0].txid()).unwrap().instant_locked = true;
    });
    resend(&f, None).unwrap();
    assert!(sweep_out_keys(&f).is_empty(), "InstantSend-locked");
    drop(f);

    let f = fixture(false);
    f.credit_coinjoin(1, COIN);
    f.credit_coinjoin(2, COIN);
    let wire = hook(&f, 500, &[Outcome::Unknown]);
    move_to_wallet(&f).unwrap();
    let sent = wire.txs();
    forget_claims(&f, &sent);
    let ins: Vec<OutPoint> = sent[0].input.iter().map(|i| i.previous_output).collect();
    mine_conflict(&f, ins[0]);
    chainlock(&f, 900);
    let wire = hook(&f, 500, &[]);
    assert_eq!(move_to_wallet(&f).unwrap().failure_code, None);
    assert_eq!(inputs(&wire.txs()), HashSet::from([ins[1]]));
    assert!(!sweep_out_keys(&f).contains(&sent[0].txid()));
}

/// Where each sweep record stands. Pending (held): neither in a block of
/// the active chain nor InstantSend-locked, nor beaten to an input in one;
/// the wallet's own mempool record, a mempool conflict, and a record or
/// beaten row whose block the chain replaced or has no header for are no
/// evidence. Resolved (kept, not held): in a block of the active chain
/// above the best ChainLock, or beaten there. Final (dropped): in a
/// ChainLocked block of the active chain (promoted, or at or below the
/// best ChainLock's height), InstantSend-locked (flag or context), or
/// beaten so. Rows that do not decode, whose bytes are not the transaction
/// the key names, that record a beaten chunk with no record, or whose
/// winner's block the chain replaced are dropped too.
#[test]
fn a_sweep_record_is_dropped_only_when_its_resolution_is_final() {
    use crate::coinjoin::{
        SweepChain, SweepOut, SweepOutState, SweepRows, held_inputs, split_sweep_outs,
        sweep_out_states,
    };
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
    let hash = |h: u32| BlockHash::from_byte_array([h as u8; 32]);
    let stale = |h: u32| BlockHash::from_byte_array([h as u8 + 100; 32]);
    let winner = Txid::from_byte_array([9; 32]);
    let beaten = |o: &SweepOut, v: &str| {
        (
            format!("coinjoin.sweep_beaten.{}", o.txid()),
            format!("{v} {winner}"),
        )
    };
    let mined_at = |h: u32, b: BlockHash| format!("{h} {b}");
    let block = |h: u32| TransactionContext::InBlock(BlockInfo::new(h, hash(h), 0));
    let rival = |n: u8, lock_time: u32| {
        let mut tx = conflict(op(n));
        tx.lock_time = lock_time;
        tx
    };
    let pending = chunk(&[1, 2]);
    let mined = chunk(&[3]);
    let mined_low = chunk(&[10]);
    let promoted = chunk(&[11]);
    let mined_conflict = chunk(&[4, 5]);
    let mempool_conflict = chunk(&[6]);
    let locked_conflict = chunk(&[12]);
    let locked = chunk(&[7]);
    let local = chunk(&[8]);
    let beaten_high = chunk(&[13]);
    let beaten_low = chunk(&[14]);
    let beaten_locked = chunk(&[15]);
    let locked_context = chunk(&[17]);
    let promoted_conflict = chunk(&[18]);
    let replaced = chunk(&[19]);
    let replaced_promoted = chunk(&[20]);
    let replaced_conflict = chunk(&[21]);
    let headerless = chunk(&[22]);
    let beaten_replaced = chunk(&[23]);
    let beaten_headerless = chunk(&[24]);
    let orphan = beaten(&chunk(&[16]), &mined_at(5, hash(5)));
    let bad = ("coinjoin.sweep_out.nope".to_string(), "00".to_string());
    let renamed = (pending.key(), mined.value());

    let mut txs = std::collections::HashMap::new();
    let mut put = |tx: Transaction, context, is: bool| {
        let mut e = history_entry(tx, context);
        e.instant_locked = is;
        txs.insert(e.tx.txid(), e);
    };
    put(mined.tx.clone(), block(20), false);
    put(mined_low.tx.clone(), block(9), false);
    put(
        promoted.tx.clone(),
        TransactionContext::InChainLockedBlock(BlockInfo::new(30, hash(30), 0)),
        false,
    );
    put(locked.tx.clone(), TransactionContext::Mempool, true);
    put(local.tx.clone(), TransactionContext::Mempool, false);
    put(rival(5, 1), block(20), false);
    put(rival(6, 2), TransactionContext::Mempool, false);
    put(rival(12, 3), TransactionContext::Mempool, true);
    put(
        locked_context.tx.clone(),
        TransactionContext::InstantSend(dashcore::InstantLock::default()),
        false,
    );
    put(
        rival(18, 4),
        TransactionContext::InChainLockedBlock(BlockInfo::new(30, hash(30), 0)),
        false,
    );
    // Reloaded from wallet.sqlite after a fork: their blocks are gone.
    put(
        replaced.tx.clone(),
        TransactionContext::InBlock(BlockInfo::new(21, stale(21), 0)),
        false,
    );
    put(
        replaced_promoted.tx.clone(),
        TransactionContext::InChainLockedBlock(BlockInfo::new(8, stale(8), 0)),
        false,
    );
    put(
        rival(21, 5),
        TransactionContext::InBlock(BlockInfo::new(22, stale(22), 0)),
        false,
    );
    put(headerless.tx.clone(), block(40), false);

    let rows = SweepRows {
        records: vec![
            row(&pending),
            row(&mined),
            row(&mined_low),
            row(&promoted),
            row(&mined_conflict),
            row(&mempool_conflict),
            row(&locked_conflict),
            row(&locked),
            row(&local),
            row(&beaten_high),
            row(&beaten_low),
            row(&beaten_locked),
            row(&locked_context),
            row(&promoted_conflict),
            row(&replaced),
            row(&replaced_promoted),
            row(&replaced_conflict),
            row(&headerless),
            row(&beaten_replaced),
            row(&beaten_headerless),
            bad.clone(),
        ],
        beaten: vec![
            beaten(&beaten_high, &mined_at(20, hash(20))),
            beaten(&beaten_low, &mined_at(9, hash(9))),
            beaten(&beaten_locked, "is"),
            beaten(&beaten_replaced, &mined_at(21, stale(21))),
            beaten(&beaten_headerless, &mined_at(41, hash(41))),
            orphan.clone(),
        ],
    };
    // The header store holds every height but 40 and 41.
    let chain = |chainlock| SweepChain {
        chainlock,
        active: [5, 8, 9, 20, 21, 22, 30]
            .into_iter()
            .map(|h| (h, hash(h)))
            .collect(),
    };
    let (states, mut gone) = sweep_out_states(rows.clone(), &txs, &chain(Some(10)));
    let state = |o: &SweepOut| {
        (states.iter())
            .find(|(s, _)| s.txid() == o.txid())
            .map(|(_, st)| *st)
    };
    use SweepOutState::{Final, Pending, Resolved};
    for (o, want) in [
        (&pending, Pending),
        (&mined, Resolved),
        (&mined_low, Final),
        (&promoted, Final),
        (&mined_conflict, Resolved),
        (&mempool_conflict, Pending),
        (&locked_conflict, Final),
        (&locked, Final),
        (&local, Pending),
        (&beaten_high, Resolved),
        (&beaten_low, Final),
        (&beaten_locked, Final),
        (&locked_context, Final),
        (&promoted_conflict, Final),
        (&replaced, Pending),
        (&replaced_promoted, Pending),
        (&replaced_conflict, Pending),
        (&headerless, Pending),
        (&beaten_replaced, Pending),
        (&beaten_headerless, Pending),
    ] {
        assert_eq!(state(o), Some(want), "{}", o.txid());
    }
    gone.sort();
    let mut want = vec![
        mined_low.key(),
        promoted.key(),
        locked_conflict.key(),
        locked.key(),
        beaten_low.key(),
        beaten(&beaten_low, "").0,
        beaten_locked.key(),
        beaten(&beaten_locked, "").0,
        locked_context.key(),
        promoted_conflict.key(),
        beaten(&beaten_replaced, "").0,
        orphan.0,
        bad.0,
    ];
    want.sort();
    assert_eq!(gone, want);

    let (held, _) = split_sweep_outs(rows, &txs, &chain(Some(10)));
    assert_eq!(
        held,
        vec![
            pending.clone(),
            mempool_conflict.clone(),
            local.clone(),
            replaced.clone(),
            replaced_promoted.clone(),
            replaced_conflict.clone(),
            headerless.clone(),
            beaten_replaced.clone(),
            beaten_headerless.clone(),
        ]
    );
    assert_eq!(
        held_inputs(&held),
        [1, 2, 6, 8, 19, 20, 21, 22, 23, 24].map(op).into()
    );
    let renamed = SweepRows {
        records: vec![renamed.clone()],
        beaten: Vec::new(),
    };
    assert_eq!(
        split_sweep_outs(renamed.clone(), &txs, &chain(None)).1,
        vec![renamed.records[0].0.clone()]
    );

    // With no ChainLock, only the promoted record and the InstantSend locks
    // are final.
    let rows = SweepRows {
        records: vec![row(&mined_low), row(&beaten_low)],
        beaten: vec![beaten(&beaten_low, &mined_at(9, hash(9)))],
    };
    let (states, gone) = sweep_out_states(rows, &txs, &chain(None));
    assert!(states.iter().all(|(_, s)| *s == Resolved));
    assert!(gone.is_empty());

    // No header store at all: nothing in a block is evidence.
    let rows = SweepRows {
        records: vec![row(&mined), row(&promoted), row(&beaten_high)],
        beaten: vec![beaten(&beaten_high, &mined_at(20, hash(20)))],
    };
    let none = SweepChain {
        chainlock: Some(100),
        active: Default::default(),
    };
    let (states, gone) = sweep_out_states(rows, &txs, &none);
    assert!(states.iter().all(|(_, s)| *s == Pending));
    assert!(gone.is_empty());
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

/// Ancestor limits: a chunk spends confirmed coins only. An
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

/// A move runs only on the plan the user confirmed. A
/// coin that arrives after the plan, the total still under the grant's
/// cap, refuses it (`PlanChanged`) before anything is signed, sent,
/// recorded or reserved. A quote is used once, an unknown or replaced one
/// is refused (and leaves the wallet's last quote usable), and a fresh
/// plan then moves everything.
#[test]
fn a_move_refuses_coins_other_than_the_confirmed_plans() {
    for consolidation in [false, true] {
        let f = fixture(false);
        f.credit_coinjoin(1, COIN);
        f.credit_coinjoin(2, COIN);
        let wire = hook(&f, 500, &[]);
        let quote = |f: &Fixture| {
            if consolidation {
                consolidation_quote(f)
            } else {
                move_quote(f)
            }
        };
        let run = |f: &Fixture, q: String| {
            if consolidation {
                consolidate_with(f, q)
            } else {
                move_with(f, q)
            }
        };
        let changed = |r: Result<SweepResult, EngineError>| {
            assert!(
                matches!(r, Err(EngineError::CoinJoin(CoinJoinFailure::PlanChanged))),
                "{r:?}"
            );
            assert!(wire.txs().is_empty());
            assert!(sweep_out_keys(&f).is_empty());
            assert!(f.session.spends.snapshot(&f.wallet).is_empty());
        };
        let confirmed = quote(&f);
        f.credit_coinjoin(3, COIN);
        changed(run(&f, confirmed.clone()));
        changed(run(&f, confirmed));
        changed(run(&f, "not a quote".into()));

        let replaced = quote(&f);
        let last = quote(&f);
        changed(run(&f, replaced));
        let done = run(&f, last).unwrap();
        assert_eq!(done.failure_code, None);
        assert_eq!(inputs(&wire.txs()).len(), 3, "the three coins");
    }
}

/// A consolidation that buys the drain nothing is refused before
/// anything is signed: the coins fit one shield lock (50 inputs), or its
/// chunks would leave as many coins as it takes.
#[test]
fn a_pointless_consolidation_is_refused() {
    let f = fixture(false);
    for n in 1..=6 {
        f.credit_coinjoin(n, COIN);
    }
    let nothing = |r: Result<SweepResult, EngineError>| {
        assert!(
            matches!(
                r,
                Err(EngineError::CoinJoin(CoinJoinFailure::NothingToMove))
            ),
            "{r:?}"
        );
    };
    let wire = hook(&f, 500, &[]);
    f.session.coinjoin.sweep_hooks.lock().unwrap().lock_inputs = None;
    let plan = (f.engine).block_on(f.session.mixed_coins_consolidation_plan(f.wallet));
    assert!(plan.is_err(), "six coins fit one shield lock: {plan:?}");
    nothing(consolidate(&f));
    // Over the lock's cap, but one coin per chunk: no fewer coins.
    hook(&f, 1, &[]);
    nothing(consolidate(&f));
    assert!(wire.txs().is_empty());
    // Over the cap, and fewer coins after: it runs.
    let wire = hook(&f, 3, &[]);
    assert_eq!(consolidate(&f).unwrap().failure_code, None);
    assert_eq!(wire.txs().len(), 2);
}

/// A move reports what arrives and the fee apart; `moved` is their
/// sum, the coins' value that left the account.
#[test]
fn a_moves_result_says_what_arrives_and_the_fee() {
    let f = fixture(false);
    for n in 1..=4 {
        f.credit_coinjoin(n, COIN);
    }
    let wire = hook(&f, 2, &[Outcome::Sent, Outcome::Rejected]);
    let r = move_to_wallet(&f).unwrap();
    let sent = &wire.txs()[0];
    let paid: u64 = sent.output.iter().map(|o| o.value).sum();
    assert_eq!((r.moved, r.received), (2 * COIN, paid));
    assert_eq!(r.fee, 2 * COIN - paid);
    assert!(r.fee > 0);
}
