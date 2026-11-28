//! #377: a coin this wallet signed into a CoinJoin round, before the
//! round's final transaction is seen. The wallet side of a session is the
//! engine's own (`EngineMixingWallet`: the coins `dsi` locks, the `dsf`
//! signature); the masternode's side is dw-coinjoin's (`tests/session.rs`:
//! a success, or a stop, timeout or dropped connection after `dss`, keeps
//! the inputs held). The production wallet manager and store hold the coin,
//! so a restart reloads it as the app would.
//!
//! A CoinJoin-account coin that is not fully mixed has two spenders: the
//! next mixing session and "Move mixed coins". Coin control and the
//! fully-mixed send page take none of it.

use std::sync::{Arc, Mutex};

use dashcore::{OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid};
use dw_coinjoin::client::MixingWallet;
use dw_vault::{Credential, GrantPurpose};

use super::own_spend_disk_tests::{FUNDED_AT, TIP, foreign, funding};
use super::{engine_wallet, mempool, mined, reopen, rows};
use crate::coinjoin::SweepDestination;
use crate::coinjoin::sweep_test_hooks::{Broadcast, Hooks};
use crate::history::TxStatusKind;
use crate::send::CoinSource;
use crate::session::NetworkSession;
use crate::{CoinFilter, Engine, WalletId};

/// The 0.1 DASH denomination.
const DENOM: u64 = 10_000_100;

/// Where the wallet offers `coin`: free in the coin list, among the next
/// mixing session's inputs, and what "Move mixed coins" would move.
#[derive(Debug, PartialEq, Eq)]
struct Offered {
    listed_free: bool,
    mixing: bool,
    move_total: u64,
}

const EVERYWHERE: Offered = Offered {
    listed_free: true,
    mixing: true,
    move_total: DENOM,
};
const NOWHERE: Offered = Offered {
    listed_free: false,
    mixing: false,
    move_total: 0,
};

fn offered(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    coin: OutPoint,
) -> Offered {
    let listed = engine
        .block_on(session.utxos(
            id,
            CoinFilter {
                include_locked: true,
                ..Default::default()
            },
        ))
        .unwrap();
    let listed_free = listed
        .iter()
        .any(|c| c.outpoint == coin && !c.reserved && !c.user_locked);
    let s = Arc::clone(session);
    let mixing = engine
        .block_on(session.on_runtime(async move {
            let (wallet, _) = s.mixing_wallet_for_test(id);
            let ready = wallet.ready_to_mix(DENOM).await.unwrap();
            Ok(ready.iter().any(|c| c.outpoint == coin))
        }))
        .unwrap();
    let move_total = engine
        .block_on(session.mixed_coins_sweep_plan(id, SweepDestination::Wallet))
        .map(|p| p.total)
        .unwrap_or(0);
    Offered {
        listed_free,
        mixing,
        move_total,
    }
}

/// Neither coin control nor the fully-mixed send page takes a coin of
/// the CoinJoin account that is not fully mixed, whatever its claims.
fn assert_no_payment_takes(
    session: &Arc<NetworkSession>,
    engine: &Engine,
    id: WalletId,
    coin: OutPoint,
) {
    for source in [
        CoinSource::Outpoints(vec![coin]),
        CoinSource::FullyMixedOnly,
    ] {
        let draft = session.new_tx_draft(id).unwrap();
        draft
            .set_recipients(vec![crate::send::Recipient {
                address: "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n".into(),
                amount: DENOM / 2,
                subtract_fee_from_amount: false,
                label: None,
                message: None,
            }])
            .unwrap();
        draft.set_source(source.clone()).unwrap();
        let r = engine.block_on(draft.estimate());
        assert!(r.is_err(), "{source:?} took it: {r:?}");
    }
}

/// A confirmed [`DENOM`] coin of the CoinJoin account, mined at
/// [`FUNDED_AT`] and scanned to [`TIP`].
fn denomination(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId) -> OutPoint {
    let s = Arc::clone(session);
    let to = engine
        .block_on(
            session.on_runtime(async move { s.reserve_coinjoin_addresses(id, 1, false).await }),
        )
        .unwrap();
    let paid = funding(41, &to, DENOM);
    mined(engine, session, id, FUNDED_AT, vec![paid.clone()]);
    mined(engine, session, id, TIP, vec![foreign()]);
    OutPoint::new(paid.txid(), 0)
}

/// A session's round, from the wallet's side: its `dsi` locks `coin`
/// (`stage` reads the wallet there), its `dsf` brings the final
/// transaction (our coin, one foreign input; our denomination, one
/// foreign), which the wallet signs (`stage` again), and the session ends
/// with our signature out, its inputs held as `spawn_session` holds them
/// (`stage` a third time). Returns the round as the masternode would
/// broadcast it.
fn sign_into_round(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    coin: OutPoint,
    ended_at: u64,
    stage: impl Fn(&str),
) -> Transaction {
    let (wallet, owner) = session.mixing_wallet_for_test(id);
    let wallet = Arc::new(wallet);
    let w = Arc::clone(&wallet);
    let (script, ours) = engine
        .block_on(session.on_runtime(async move {
            let ready = w.ready_to_mix(DENOM).await.unwrap();
            let picked = (ready.into_iter())
                .find(|c| c.outpoint == coin)
                .expect("ready to mix");
            let ours = w.reserve_scripts(1).await.unwrap().remove(0);
            w.lock_coins(&[coin]);
            Ok((picked.script_pubkey, ours))
        }))
        .unwrap();
    stage("dsi");
    let mut round = Transaction {
        version: 1,
        lock_time: 0,
        input: vec![
            TxIn {
                previous_output: coin,
                sequence: u32::MAX,
                ..Default::default()
            },
            TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([0xf0; 32]), 3),
                sequence: u32::MAX,
                ..Default::default()
            },
        ],
        output: vec![
            TxOut {
                value: DENOM,
                script_pubkey: ours,
            },
            TxOut {
                value: DENOM,
                script_pubkey: ScriptBuf::from_bytes(
                    [&[0x76, 0xa9, 0x14][..], &[0xf0; 20], &[0x88, 0xac]].concat(),
                ),
            },
        ],
        special_transaction_payload: None,
    };
    let w = Arc::clone(&wallet);
    let unsigned = round.clone();
    let signed =
        engine
            .block_on(session.on_runtime(async move {
                Ok(w.sign_inputs(&unsigned, &[(0, script)]).await.unwrap())
            }))
            .unwrap();
    assert!(
        !signed[0].script_sig.is_empty(),
        "the wallet signed its input"
    );
    round.input[0] = signed[0].clone();
    stage("dss");
    session.hold_signed_for_test(id, owner, &[coin], ended_at);
    session.expire_held_for_test(id);
    stage("the session's end");
    round
}

fn return_held(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    answer: fn(&Transaction) -> Result<Txid, Box<platform_wallet::PlatformWalletError>>,
) -> (crate::coinjoin::SweepResult, Vec<Transaction>) {
    let wire = Arc::new(Mutex::new(Vec::new()));
    let w = Arc::clone(&wire);
    let broadcast: Broadcast = Arc::new(move |tx| {
        w.lock().unwrap().push(tx.clone());
        answer(tx)
    });
    *session.coinjoin.sweep_hooks.lock().unwrap() = Hooks {
        broadcast: Some(broadcast),
        ..Hooks::default()
    };
    let quote = engine
        .block_on(session.return_from_mixing_plan(id))
        .unwrap()
        .quote_id;
    let grant = session
        .vault()
        .authorize(
            GrantPurpose::Spend {
                max_duffs: 10 * DENOM,
            },
            Some(&id.0),
            Credential::None,
        )
        .unwrap()
        .id;
    let result = engine
        .block_on(session.return_from_mixing(id, quote, grant))
        .unwrap();
    let sent = wire.lock().unwrap().clone();
    (result, sent)
}

fn status(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    txid: Txid,
) -> Option<TxStatusKind> {
    rows(engine, session, id)
        .into_iter()
        .find(|r| r.0 == txid.to_string())
        .map(|r| r.1)
}

/// (a) The wallet stays up through the round: from `dsi` through the
/// signature and the session's end the coin is offered nowhere, and once
/// the round's transaction arrives it is gone.
#[test]
fn pin_377_a_a_coin_signed_into_a_round_is_not_offered_while_the_wallet_stays_up() {
    let (_dir, engine, session, id) = engine_wallet();
    let coin = denomination(&engine, &session, id);
    assert_eq!(offered(&engine, &session, id, coin), EVERYWHERE);
    assert_no_payment_takes(&session, &engine, id, coin);

    let round = sign_into_round(
        &engine,
        &session,
        id,
        coin,
        crate::coins::now_secs(),
        |at| {
            assert_eq!(offered(&engine, &session, id, coin), NOWHERE, "at {at}");
        },
    );

    mempool(&engine, &session, id, &round);
    session.expire_held_for_test(id);
    let after = offered(&engine, &session, id, coin);
    assert_eq!(
        (after.listed_free, after.mixing),
        (false, false),
        "the round seen"
    );
    assert!(session.spends.snapshot(&id).is_empty(), "no claim left");
}

#[test]
fn a_signed_rounds_coin_stays_held_past_the_observation_timeout() {
    let (_dir, engine, session, id) = engine_wallet();
    let coin = denomination(&engine, &session, id);
    let timeout = crate::coinjoin::PENDING_OBSERVATION_FOR_TEST;

    let ended = crate::coins::now_secs() - timeout;
    sign_into_round(&engine, &session, id, coin, ended, |_| {});
    session.expire_held_for_test(id);
    assert_eq!(
        offered(&engine, &session, id, coin),
        NOWHERE,
        "an hour after the session"
    );
}

/// A restart does not free a handed-out input. The round's block brings
/// back its output without offering the spent input again.
#[test]
fn a_coin_signed_into_a_round_stays_held_after_restart() {
    let (_dir, engine, session, id) = engine_wallet();
    let coin = denomination(&engine, &session, id);
    let round = sign_into_round(
        &engine,
        &session,
        id,
        coin,
        crate::coins::now_secs(),
        |_| {},
    );
    let session = reopen(&engine, session);
    assert_eq!(
        offered(&engine, &session, id, coin),
        NOWHERE,
        "after the restart"
    );
    assert_no_payment_takes(&session, &engine, id, coin);

    mined(&engine, &session, id, TIP + 1, vec![round.clone()]);
    let cleared = offered(&engine, &session, id, coin);
    assert_eq!(
        (cleared.listed_free, cleared.mixing),
        (false, false),
        "the round mined"
    );
    assert_eq!(cleared.move_total, DENOM, "only the round's own output");
    assert!(status(&engine, &session, id, round.txid()).is_some());

    let session = reopen(&engine, session);
    let again = offered(&engine, &session, id, coin);
    assert_eq!(
        (again.listed_free, again.mixing),
        (false, false),
        "after a second restart"
    );
    assert!(session.spends.snapshot(&id).is_empty());
}

#[test]
fn a_return_rejected_by_the_network_leaves_the_coin_held_until_the_round_is_mined() {
    let (_dir, engine, session, id) = engine_wallet();
    let coin = denomination(&engine, &session, id);
    let round = sign_into_round(
        &engine,
        &session,
        id,
        coin,
        crate::coins::now_secs(),
        |_| {},
    );
    let session = reopen(&engine, session);

    let (moved, sent) = return_held(&engine, &session, id, |_| {
        Err(Box::new(
            platform_wallet::PlatformWalletError::TransactionBroadcast(
                "rejected: txn-mempool-conflict".into(),
            ),
        ))
    });
    assert_eq!(sent.len(), 1);
    assert!(moved.failure_code.is_some(), "refused: {moved:?}");
    assert_eq!(
        offered(&engine, &session, id, coin),
        NOWHERE,
        "refused, the round unseen"
    );

    mined(&engine, &session, id, TIP + 1, vec![round]);
    let cleared = offered(&engine, &session, id, coin);
    assert_eq!(
        (cleared.listed_free, cleared.mixing),
        (false, false),
        "the round mined"
    );
}

#[test]
fn a_legacy_coin_stays_held_across_restart() {
    let (_dir, engine, session, id) = engine_wallet();
    let coin = denomination(&engine, &session, id);
    let scope = dw_appdb::local_scope(&id.to_string());
    engine
        .block_on(session.appdb_op(move |db| {
            db.set_setting_durable(&scope, crate::coinjoin::TRACKED_WALLET_KEY, None)
        }))
        .unwrap();
    assert_eq!(offered(&engine, &session, id, coin), NOWHERE);
    let session = reopen(&engine, session);
    assert_eq!(offered(&engine, &session, id, coin), NOWHERE);
    assert_no_payment_takes(&session, &engine, id, coin);
}

#[test]
fn a_coordinator_can_only_receive_the_recorded_collateral_bytes_after_restart() {
    let (_dir, engine, session, id) = engine_wallet();
    let s = Arc::clone(&session);
    let address = engine
        .block_on(
            session.on_runtime(async move { s.reserve_coinjoin_addresses(id, 1, false).await }),
        )
        .unwrap();
    let deposit = funding(51, &address, dw_coinjoin::denoms::MAX_COLLATERAL_AMOUNT);
    mined(&engine, &session, id, FUNDED_AT, vec![deposit.clone()]);
    mined(&engine, &session, id, TIP, vec![foreign()]);
    let coin = OutPoint::new(deposit.txid(), 0);
    let collateral = Transaction {
        version: 1,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: coin,
            ..Default::default()
        }],
        output: vec![TxOut {
            value: 0,
            script_pubkey: ScriptBuf::from_bytes(vec![0x6a]),
        }],
        special_transaction_payload: None,
    };
    let handoff = |session: &Arc<NetworkSession>, tx: Transaction| {
        let (wallet, _) = session.mixing_wallet_for_test(id);
        engine
            .block_on(session.on_runtime(async move {
                wallet.lock_coins(&[coin]);
                let result = wallet.handoff_collateral(&tx).await;
                wallet.unlock_coins(&[coin]);
                Ok(result)
            }))
            .unwrap()
    };
    assert!(handoff(&session, collateral.clone()).is_ok());
    let session = reopen(&engine, session);
    assert_eq!(offered(&engine, &session, id, coin), NOWHERE);
    let changed = Transaction {
        lock_time: 1,
        ..collateral.clone()
    };
    assert!(handoff(&session, changed).is_err());
    assert!(handoff(&session, collateral).is_ok());
}

#[test]
fn returning_to_core_spends_only_the_held_coins_and_waits_for_confirmation() {
    use crate::send::FeeMode;
    let (_dir, engine, session, id) = engine_wallet();
    let coin = denomination(&engine, &session, id);
    sign_into_round(
        &engine,
        &session,
        id,
        coin,
        crate::coins::now_secs(),
        |_| {},
    );
    let another = denomination(&engine, &session, id);
    assert_ne!(another, coin);
    let (returned, sent) = return_held(&engine, &session, id, |tx| Ok(tx.txid()));
    assert_eq!(returned.failure_code, None);
    let [tx] = sent.as_slice() else {
        panic!("expected one return transaction")
    };
    assert_eq!(
        tx.input
            .iter()
            .map(|i| i.previous_output)
            .collect::<Vec<_>>(),
        [coin]
    );
    assert_eq!(tx.output.len(), 1);
    assert_eq!(tx.output[0].value + returned.fee, DENOM);
    let core = || {
        engine
            .block_on(session.max_spendable(
                id,
                CoinSource::Any,
                FeeMode::Recommended { target_blocks: 6 },
            ))
            .unwrap()
    };
    mempool(&engine, &session, id, tx);
    assert_eq!(core(), 0);
    mined(&engine, &session, id, TIP + 1, vec![tx.clone()]);
    assert_eq!(core(), returned.received);
}

/// What a session asks of the real wallet before it submits or signs a
/// coin (`any_withdrawn`, `unspent_outpoints`): a coin it locked itself is
/// still its own and still unspent; a user's "Lock unspent", or another
/// owner's claim, withdraws it. Every 1:1 session runs these, so a coin
/// read that dropped reserved coins would refuse every honest round.
#[test]
fn a_sessions_own_coin_stays_its_own_and_a_lock_or_claim_withdraws_it() {
    let (_dir, engine, session, id) = engine_wallet();
    let coin = denomination(&engine, &session, id);
    let (wallet, _owner) = session.mixing_wallet_for_test(id);
    let wallet = Arc::new(wallet);
    let ask = || {
        let w = Arc::clone(&wallet);
        engine
            .block_on(session.on_runtime(async move {
                let withdrawn = w.any_withdrawn(&[coin]).await.unwrap();
                let unspent = w.unspent_outpoints().await.unwrap().contains(&coin);
                Ok((withdrawn, unspent))
            }))
            .unwrap()
    };
    wallet.lock_coins(&[coin]);
    assert_eq!(ask(), (false, true), "locked by the session itself");

    let other = crate::send::SpendOwner::mixing_session();
    session.spends.reserve(id, other, [coin]);
    assert!(ask().0, "another owner's claim");
    session.spends.release_some(&id, other, [coin]);
    assert!(!ask().0, "the claim released");

    engine
        .block_on(session.lock_outpoints(id, vec![coin]))
        .unwrap();
    assert!(ask().0, "the user locked it");
    wallet.unlock_coins(&[coin]);
}

/// A "Lock unspent" that lands after the session's last check and before
/// its signature (the fence is first come, first served) still keeps the
/// coin from being signed: the signer checks again under its own fence.
#[test]
fn a_coin_the_user_locks_after_the_check_is_not_signed() {
    let (_dir, engine, session, id) = engine_wallet();
    let coin = denomination(&engine, &session, id);
    let (wallet, _owner) = session.mixing_wallet_for_test(id);
    let wallet = Arc::new(wallet);
    let w = Arc::clone(&wallet);
    let (script, ours) = engine
        .block_on(session.on_runtime(async move {
            let ready = w.ready_to_mix(DENOM).await.unwrap();
            let picked = (ready.into_iter()).find(|c| c.outpoint == coin).unwrap();
            let ours = w.reserve_scripts(1).await.unwrap().remove(0);
            w.lock_coins(&[coin]);
            assert!(!w.any_withdrawn(&[coin]).await.unwrap());
            Ok((picked.script_pubkey, ours))
        }))
        .unwrap();
    engine
        .block_on(session.lock_outpoints(id, vec![coin]))
        .unwrap();
    let round = Transaction {
        version: 1,
        lock_time: 0,
        input: vec![
            TxIn {
                previous_output: coin,
                sequence: u32::MAX,
                ..Default::default()
            },
            TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([0xf0; 32]), 3),
                sequence: u32::MAX,
                ..Default::default()
            },
        ],
        output: vec![
            TxOut {
                value: DENOM,
                script_pubkey: ours,
            },
            TxOut {
                value: DENOM,
                script_pubkey: ScriptBuf::from_bytes(
                    [&[0x76, 0xa9, 0x14][..], &[0xf0; 20], &[0x88, 0xac]].concat(),
                ),
            },
        ],
        special_transaction_payload: None,
    };
    let w = Arc::clone(&wallet);
    let signed = engine
        .block_on(
            session.on_runtime(async move { Ok(w.sign_inputs(&round, &[(0, script)]).await) }),
        )
        .unwrap();
    assert!(signed.is_err(), "a coin the user locked was signed");
    wallet.unlock_coins(&[coin]);
}

#[test]
fn mixing_session_held_coins_keep_coinjoin_total_steady() {
    let (_dir, engine, session, id) = engine_wallet();
    let s = Arc::clone(&session);
    let addrs = engine
        .block_on(
            session.on_runtime(async move { s.reserve_coinjoin_addresses(id, 4, false).await }),
        )
        .unwrap();
    let paid = funding(42, &addrs, DENOM);
    mined(&engine, &session, id, FUNDED_AT, vec![paid.clone()]);
    mined(&engine, &session, id, TIP, vec![foreign()]);
    let coins: Vec<OutPoint> = (0..4).map(|v| OutPoint::new(paid.txid(), v)).collect();

    let read_status = || {
        let s = Arc::clone(&session);
        engine
            .block_on(session.on_runtime(async move {
                s.refresh_coinjoin_status(id).await;
                Ok(())
            }))
            .unwrap();
        session.coinjoin_status(id).unwrap()
    };

    let before = read_status();
    assert_eq!(before.balances.anonymizable, 4 * DENOM);
    assert_eq!(before.amount_and_rounds.amount, 4 * DENOM);

    let (wallet, _owner) = session.mixing_wallet_for_test(id);
    wallet.lock_coins(&coins[..2]);
    let while_partial = read_status();
    assert_eq!(while_partial.balances.anonymizable, 4 * DENOM);
    assert_eq!(while_partial.amount_and_rounds.amount, 4 * DENOM);
    assert_eq!(while_partial.progress, before.progress);

    let ready_partial = engine
        .block_on(session.on_runtime({
            let s = Arc::clone(&session);
            async move {
                let (w, _) = s.mixing_wallet_for_test(id);
                Ok(w.ready_to_mix(DENOM).await.unwrap())
            }
        }))
        .unwrap();
    assert_eq!(ready_partial.len(), 2);
    for held in &coins[..2] {
        assert!(!ready_partial.iter().any(|c| c.outpoint == *held));
    }

    wallet.lock_coins(&coins[2..]);
    let while_all = read_status();
    assert_eq!(while_all.balances.anonymizable, 4 * DENOM);
    assert_eq!(while_all.amount_and_rounds.amount, 4 * DENOM);
    assert_eq!(while_all.progress, before.progress);

    let tx_owner = crate::send::SpendOwner::Tx(Txid::from_byte_array([0xab; 32]));
    session.spends.reserve(id, tx_owner, [coins[0]]);
    let with_pending_tx = read_status();
    assert_eq!(with_pending_tx.balances.anonymizable, 3 * DENOM);
    assert_eq!(with_pending_tx.amount_and_rounds.amount, 3 * DENOM);
    session.spends.release(&id, tx_owner);

    engine
        .block_on(session.lock_outpoints(id, vec![coins[1]]))
        .unwrap();
    let with_user_lock = read_status();
    assert_eq!(with_user_lock.balances.anonymizable, 3 * DENOM);
    assert_eq!(with_user_lock.amount_and_rounds.amount, 3 * DENOM);
    engine
        .block_on(session.unlock_outpoints(id, vec![coins[1]]))
        .unwrap();

    wallet.unlock_coins(&coins);
    let after = read_status();
    assert_eq!(after.balances.anonymizable, 4 * DENOM);
    assert_eq!(after.amount_and_rounds.amount, 4 * DENOM);
    assert_eq!(after.progress, before.progress);
}

#[test]
fn held_from_mixing_counts_legacy_and_handoff_coins_and_clears_after_return() {
    let (_dir, engine, session, id) = engine_wallet();
    let free_coin = denomination(&engine, &session, id);
    let held_denom = denomination(&engine, &session, id);
    assert_ne!(free_coin, held_denom);

    let read_status = || {
        let s = Arc::clone(&session);
        engine
            .block_on(session.on_runtime(async move {
                s.refresh_coinjoin_status(id).await;
                Ok(())
            }))
            .unwrap();
        session.coinjoin_status(id).unwrap()
    };

    // A free CoinJoin coin alongside a FinalInputs-held coin: only the
    // held coin counts in `held_from_mixing`, never the free coin.
    sign_into_round(
        &engine,
        &session,
        id,
        held_denom,
        crate::coins::now_secs(),
        |_| {},
    );
    let mixed_state = read_status();
    assert_eq!(mixed_state.balances.coinjoin_account, DENOM);
    assert_eq!(mixed_state.balances.held_from_mixing, DENOM);

    let (returned_handoff, sent_handoff) = return_held(&engine, &session, id, |tx| Ok(tx.txid()));
    assert_eq!(returned_handoff.failure_code, None);
    assert_eq!(sent_handoff.len(), 1);
    assert_eq!(sent_handoff[0].input.len(), 1);
    assert_eq!(sent_handoff[0].input[0].previous_output, held_denom);
    mined(&engine, &session, id, TIP + 1, sent_handoff);

    let after_handoff = read_status();
    assert_eq!(after_handoff.balances.held_from_mixing, 0);
    assert_eq!(after_handoff.balances.coinjoin_account, DENOM);

    // Untracked/legacy wallet: both the remaining denomination and a
    // collateral coin are held as legacy coins, and locking one removes it
    // from `held_from_mixing` until unlocked and returned.
    let s = Arc::clone(&session);
    let col_addr = engine
        .block_on(
            session.on_runtime(async move { s.reserve_coinjoin_addresses(id, 1, false).await }),
        )
        .unwrap();
    let col_tx = funding(52, &col_addr, dw_coinjoin::denoms::MAX_COLLATERAL_AMOUNT);
    mined(&engine, &session, id, TIP + 2, vec![col_tx.clone()]);
    mined(&engine, &session, id, TIP + 3, vec![foreign()]);
    let col = OutPoint::new(col_tx.txid(), 0);

    let scope = dw_appdb::local_scope(&id.to_string());
    engine
        .block_on(session.appdb_op(move |db| {
            db.set_setting_durable(&scope, crate::coinjoin::TRACKED_WALLET_KEY, None)
        }))
        .unwrap();

    let held = read_status();
    assert_eq!(held.balances.coinjoin_account, 0);
    assert_eq!(
        held.balances.held_from_mixing,
        DENOM + dw_coinjoin::denoms::MAX_COLLATERAL_AMOUNT
    );

    engine
        .block_on(session.lock_outpoints(id, vec![col]))
        .unwrap();
    assert_eq!(read_status().balances.held_from_mixing, DENOM);
    engine
        .block_on(session.unlock_outpoints(id, vec![col]))
        .unwrap();

    let (returned, sent) = return_held(&engine, &session, id, |tx| Ok(tx.txid()));
    assert_eq!(returned.failure_code, None);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].input.len(), 2);
    assert!(sent[0].input.iter().any(|i| i.previous_output == free_coin));
    assert!(sent[0].input.iter().any(|i| i.previous_output == col));
    mined(&engine, &session, id, TIP + 4, sent);

    let cleared = read_status();
    assert_eq!(cleared.balances.held_from_mixing, 0);
    assert_eq!(cleared.balances.coinjoin_account, 0);
}

#[test]
fn return_from_mixing_pays_distinct_unissued_bip44_addresses_per_chunk() {
    let (_dir, engine, session, id) = engine_wallet();
    let shared = engine
        .block_on(session.next_receive_address(id, None))
        .unwrap();
    let shared_script = shared
        .address
        .parse::<dashcore::Address<dashcore::address::NetworkUnchecked>>()
        .unwrap()
        .assume_checked()
        .script_pubkey();

    let a = denomination(&engine, &session, id);
    let b = denomination(&engine, &session, id);
    assert_ne!(a, b);
    let scope = dw_appdb::local_scope(&id.to_string());
    engine
        .block_on(session.appdb_op(move |db| {
            db.set_setting_durable(&scope, crate::coinjoin::TRACKED_WALLET_KEY, None)
        }))
        .unwrap();

    let wire = Arc::new(Mutex::new(Vec::new()));
    let w = Arc::clone(&wire);
    let broadcast: Broadcast = Arc::new(move |tx| {
        w.lock().unwrap().push(tx.clone());
        Ok(tx.txid())
    });
    *session.coinjoin.sweep_hooks.lock().unwrap() = Hooks {
        max_inputs: Some(1),
        broadcast: Some(broadcast),
        ..Hooks::default()
    };
    let plan = engine
        .block_on(session.return_from_mixing_plan(id))
        .unwrap();
    assert_eq!(plan.chunks.len(), 2);
    let grant = session
        .vault()
        .authorize(
            GrantPurpose::Spend {
                max_duffs: 10 * DENOM,
            },
            Some(&id.0),
            Credential::None,
        )
        .unwrap()
        .id;
    let result = engine
        .block_on(session.return_from_mixing(id, plan.quote_id, grant))
        .unwrap();
    assert_eq!(result.failure_code, None);
    let sent = wire.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    let out0 = &sent[0].output[0].script_pubkey;
    let out1 = &sent[1].output[0].script_pubkey;
    assert_ne!(
        out0, &shared_script,
        "chunk 0 reused an issued receive address"
    );
    assert_ne!(
        out1, &shared_script,
        "chunk 1 reused an issued receive address"
    );
    assert_ne!(
        out0, out1,
        "multi-chunk return reused the same receive address"
    );
}

#[test]
fn return_from_mixing_falls_back_to_distinct_available_addresses_at_gap_limit() {
    let (_dir, engine, session, id) = engine_wallet();
    let mut issued_scripts = std::collections::HashSet::new();
    loop {
        match engine.block_on(session.next_receive_address(id, None)) {
            Ok(info) => {
                let script = info
                    .address
                    .parse::<dashcore::Address<dashcore::address::NetworkUnchecked>>()
                    .unwrap()
                    .assume_checked()
                    .script_pubkey();
                issued_scripts.insert(script);
            }
            Err(crate::EngineError::GapLimit) => break,
            Err(e) => panic!(
                "unexpected error issuing address #{}: {e:?}",
                issued_scripts.len()
            ),
        }
    }
    assert!(
        issued_scripts.len() >= 20,
        "expected full receive gap limit, got {}",
        issued_scripts.len()
    );

    let a = denomination(&engine, &session, id);
    let b = denomination(&engine, &session, id);
    assert_ne!(a, b);
    let scope = dw_appdb::local_scope(&id.to_string());
    engine
        .block_on(session.appdb_op(move |db| {
            db.set_setting_durable(&scope, crate::coinjoin::TRACKED_WALLET_KEY, None)
        }))
        .unwrap();

    let wire = Arc::new(Mutex::new(Vec::new()));
    let w = Arc::clone(&wire);
    let broadcast: Broadcast = Arc::new(move |tx| {
        w.lock().unwrap().push(tx.clone());
        Ok(tx.txid())
    });
    *session.coinjoin.sweep_hooks.lock().unwrap() = Hooks {
        max_inputs: Some(1),
        broadcast: Some(broadcast),
        ..Hooks::default()
    };
    let plan = engine
        .block_on(session.return_from_mixing_plan(id))
        .unwrap();
    assert_eq!(plan.chunks.len(), 2);
    let grant = session
        .vault()
        .authorize(
            GrantPurpose::Spend {
                max_duffs: 10 * DENOM,
            },
            Some(&id.0),
            Credential::None,
        )
        .unwrap()
        .id;
    let result = engine
        .block_on(session.return_from_mixing(id, plan.quote_id, grant))
        .expect("return at gap limit should fall back to available receive addresses");
    assert_eq!(result.failure_code, None);
    let sent = wire.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    let out0 = &sent[0].output[0].script_pubkey;
    let out1 = &sent[1].output[0].script_pubkey;
    assert_ne!(
        out0, out1,
        "fallback multi-chunk return reused the same receive address"
    );
    assert!(
        issued_scripts.contains(out0) && issued_scripts.contains(out1),
        "fallback return paid an address outside the wallet's receiving pool"
    );
}

#[test]
fn return_from_mixing_plan_refuses_while_restored() {
    let (_dir, engine, session, id) = engine_wallet();
    let coin = denomination(&engine, &session, id);
    sign_into_round(
        &engine,
        &session,
        id,
        coin,
        crate::coins::now_secs(),
        |_| {},
    );

    let dir = session
        .data_dir()
        .join(dw_store_upgrade::UPGRADE_BACKUPS_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("RESTORED"), "restored\nformat 1\n").unwrap();
    let session = reopen(&engine, session);
    assert!(session.restored(), "restored mark must be active");

    let err = engine
        .block_on(session.return_from_mixing_plan(id))
        .expect_err("plan must refuse while restored");
    assert!(
        matches!(err, crate::EngineError::Restored),
        "expected Restored, got {err:?}"
    );
}
