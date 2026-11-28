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
fn psbt_sign_and_broadcast_refuse_coinjoin_handoff_input() {
    let (_dir, engine, session, id) = engine_wallet();
    let make_coin = |seed: u8| {
        let s = Arc::clone(&session);
        let to = engine
            .block_on(
                session.on_runtime(async move { s.reserve_coinjoin_addresses(id, 1, false).await }),
            )
            .unwrap();
        let paid = funding(seed, &to, DENOM);
        mined(&engine, &session, id, FUNDED_AT, vec![paid.clone()]);
        mined(&engine, &session, id, TIP, vec![foreign()]);
        let outpoint = OutPoint::new(paid.txid(), 0);
        (paid, outpoint)
    };
    let psbt_of = |prev: &Transaction, outpoint: OutPoint| {
        let unsigned = Transaction {
            version: 2,
            lock_time: 0,
            input: vec![TxIn {
                previous_output: outpoint,
                ..Default::default()
            }],
            output: vec![TxOut {
                value: DENOM - 1_000,
                script_pubkey: super::elsewhere(),
            }],
            special_transaction_payload: None,
        };
        let mut psbt = dw_psbt::PartiallySignedTransaction::from_unsigned_tx(unsigned).unwrap();
        psbt.inputs[0].non_witness_utxo = Some(prev.clone());
        psbt
    };
    let grant = || {
        session
            .vault()
            .authorize(
                GrantPurpose::Spend {
                    max_duffs: 2 * DENOM,
                },
                Some(&id.0),
                Credential::None,
            )
            .unwrap()
            .id
    };
    fn assert_handoff_refused<T: std::fmt::Debug>(
        r: Result<T, crate::EngineError>,
        outpoint: OutPoint,
    ) {
        match r {
            Err(crate::EngineError::Psbt(crate::PsbtFailure::Invalid(d))) => {
                assert!(
                    d.contains(&outpoint.to_string())
                        && d.contains("held")
                        && d.contains("Return to Core"),
                    "expected held CoinJoin hand-off refusal naming coin and way out, got: {d}"
                );
            }
            other => {
                panic!("expected PsbtFailure::Invalid for held hand-off coin, got: {other:?}")
            }
        }
    }

    // Case 1: durable FinalInputs hand-off after signing into a mixing round.
    let (prev_round, coin_round) = make_coin(61);
    let psbt_round = psbt_of(&prev_round, coin_round);
    let pre_signed_round = engine
        .block_on(session.sign_psbt(id, psbt_round.clone(), grant()))
        .expect("free CoinJoin coin signs before hand-off");
    sign_into_round(
        &engine,
        &session,
        id,
        coin_round,
        crate::coins::now_secs(),
        |_| {},
    );
    let session = reopen(&engine, session);
    assert_handoff_refused(
        engine.block_on(session.sign_psbt(id, psbt_round, {
            session
                .vault()
                .authorize(
                    GrantPurpose::Spend {
                        max_duffs: 2 * DENOM,
                    },
                    Some(&id.0),
                    Credential::None,
                )
                .unwrap()
                .id
        })),
        coin_round,
    );
    assert_handoff_refused(
        engine.block_on(session.broadcast_psbt(pre_signed_round)),
        coin_round,
    );

    // Case 2: legacy CoinJoin hand-off on an untracked wallet.
    let s = Arc::clone(&session);
    let to_legacy = engine
        .block_on(
            session.on_runtime(async move { s.reserve_coinjoin_addresses(id, 1, false).await }),
        )
        .unwrap();
    let prev_legacy = funding(62, &to_legacy, DENOM);
    mined(&engine, &session, id, TIP + 1, vec![prev_legacy.clone()]);
    mined(&engine, &session, id, TIP + 2, vec![foreign()]);
    let coin_legacy = OutPoint::new(prev_legacy.txid(), 0);
    let psbt_legacy = psbt_of(&prev_legacy, coin_legacy);
    let legacy_grant = session
        .vault()
        .authorize(
            GrantPurpose::Spend {
                max_duffs: 2 * DENOM,
            },
            Some(&id.0),
            Credential::None,
        )
        .unwrap()
        .id;
    let pre_signed_legacy = engine
        .block_on(session.sign_psbt(id, psbt_legacy.clone(), legacy_grant))
        .expect("free CoinJoin coin signs before legacy classification");
    let scope = dw_appdb::local_scope(&id.to_string());
    engine
        .block_on(session.appdb_op(move |db| {
            db.set_setting_durable(&scope, crate::coinjoin::TRACKED_WALLET_KEY, None)
        }))
        .unwrap();
    let legacy_grant2 = session
        .vault()
        .authorize(
            GrantPurpose::Spend {
                max_duffs: 2 * DENOM,
            },
            Some(&id.0),
            Credential::None,
        )
        .unwrap()
        .id;
    assert_handoff_refused(
        engine.block_on(session.sign_psbt(id, psbt_legacy, legacy_grant2)),
        coin_legacy,
    );
    assert_handoff_refused(
        engine.block_on(session.broadcast_psbt(pre_signed_legacy)),
        coin_legacy,
    );
}
