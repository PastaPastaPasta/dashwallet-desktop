//! "Move mixed coins" and its consolidation (0.2 MIX-SHIELD unit A)
//! against a real session and vault: the mixing refusal, the consolidation
//! round inside the CoinJoin account, and what a restart keeps of a sweep
//! that may be on the network.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use dashcore::{OutPoint, Transaction, Txid};
use dw_vault::{Credential, GrantPurpose, is_bip44_path, is_coinjoin_path};
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use platform_wallet::PlatformWalletError as Pw;

use crate::EngineError;
use crate::coinjoin::sweep_test_hooks::Hooks;
use crate::coinjoin::{CoinJoinFailure, SweepDestination, SweepResult};
use crate::platform::lease::session_tests::{Fixture, fixture};
use crate::send::SpendOwner;

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

/// SPV saw `txs`: their inputs are no coins of the wallet any more.
fn see_spent(f: &Fixture, txs: &[Transaction]) {
    let spent: HashSet<OutPoint> = txs
        .iter()
        .flat_map(|t| t.input.iter().map(|i| i.previous_output))
        .collect();
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

/// Q3: the round pays one fresh CoinJoin-account address per chunk, never a
/// BIP44 one, takes every final coin once, in the fewest chunks under the
/// cap, and leaves the coins that are not final.
#[test]
fn a_consolidation_pays_fresh_coinjoin_addresses_and_takes_final_coins_only() {
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
    assert_eq!(plan.not_final, 1);
    assert_eq!(plan.coins_after, 4, "three outputs and the pending coin");
    assert_eq!(plan.not_fully_mixed + plan.fully_mixed, 6 * COIN);

    let result = consolidate(&f).unwrap();
    assert_eq!(result.failure_code, None);
    assert_eq!(result.txids.len(), 3);
    let txs = wire.txs();
    assert_eq!(
        inputs(&txs),
        coins.iter().copied().collect(),
        "each final coin once"
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
/// chunk 2's inputs stay held until it is seen too.
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
