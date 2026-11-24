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

use dashcore::hashes::Hash;
use dashcore::{OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid};
use dw_coinjoin::client::MixingWallet;
use dw_vault::{Credential, GrantPurpose};

use super::own_spend_disk_tests::{FUNDED_AT, TIP, abandon, foreign, funding};
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
    session.hold_signed_for_test(id, owner, &[coin], crate::coins::now_secs());
    session.expire_held_for_test(id);
    stage("the session's end");
    round
}

/// "Move mixed coins" of the whole account, its broadcast answered by
/// `answer`; returns the result and the chunks handed to the network.
fn move_mixed(
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
        .block_on(session.mixed_coins_sweep_plan(id, SweepDestination::Wallet))
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
        .block_on(session.move_mixed_coins(id, SweepDestination::Wallet, quote, grant))
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

    let round = sign_into_round(&engine, &session, id, coin, |at| {
        assert_eq!(offered(&engine, &session, id, coin), NOWHERE, "at {at}");
    });

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

/// (a), past Dash Core's observation timeout: a round whose transaction
/// the wallet has not seen an hour after the session ended frees its
/// coin (`COINJOIN_PENDING_OBSERVATION_TIMEOUT`).
#[test]
fn pin_377_a_an_unseen_round_frees_its_coin_after_the_observation_timeout() {
    let (_dir, engine, session, id) = engine_wallet();
    let coin = denomination(&engine, &session, id);
    let timeout = crate::coinjoin::PENDING_OBSERVATION_FOR_TEST;

    let (_, owner) = session.mixing_wallet_for_test(id);
    session.spends.reserve(id, owner, [coin]);
    let ended = crate::coins::now_secs() - timeout + 60;
    session.hold_signed_for_test(id, owner, &[coin], ended);
    session.expire_held_for_test(id);
    assert_eq!(
        offered(&engine, &session, id, coin),
        NOWHERE,
        "59 minutes after the session"
    );
    session.spends.release(&id, owner);

    let (_, owner) = session.mixing_wallet_for_test(id);
    session.spends.reserve(id, owner, [coin]);
    let ended = crate::coins::now_secs() - timeout;
    session.hold_signed_for_test(id, owner, &[coin], ended);
    session.expire_held_for_test(id);
    assert_eq!(
        offered(&engine, &session, id, coin),
        EVERYWHERE,
        "an hour after the session"
    );
}

/// (b) A restart after the signature, before the round's transaction
/// arrives: the coin is offered again, and "Move mixed coins" signs it
/// into a second spend and hands it to the network. The way out: Abandon
/// the move (the wallet keeps the coin spent until a rescan), then the
/// round's block clears the coin, across a restart too.
#[test]
fn pin_377_b_after_a_restart_a_coin_signed_into_a_round_is_offered_and_moved() {
    let (_dir, engine, session, id) = engine_wallet();
    let coin = denomination(&engine, &session, id);
    let round = sign_into_round(&engine, &session, id, coin, |_| {});
    let session = reopen(&engine, session);
    assert_eq!(
        offered(&engine, &session, id, coin),
        EVERYWHERE,
        "after the restart"
    );
    assert_no_payment_takes(&session, &engine, id, coin);

    let (moved, sent) = move_mixed(&engine, &session, id, |tx| Ok(tx.txid()));
    assert_eq!(moved.failure_code, None, "{moved:?}");
    assert_eq!(sent.len(), 1);
    let chunk = sent[0].clone();
    assert_eq!(
        chunk
            .input
            .iter()
            .map(|i| i.previous_output)
            .collect::<Vec<_>>(),
        vec![coin],
        "a second spend of the coin"
    );
    assert_ne!(chunk.txid(), round.txid());
    mempool(&engine, &session, id, &chunk);
    assert_eq!(
        offered(&engine, &session, id, coin),
        NOWHERE,
        "the move holds it"
    );

    abandon(&engine, &session, id, chunk.txid());
    assert_eq!(
        status(&engine, &session, id, chunk.txid()),
        Some(TxStatusKind::Abandoned)
    );
    let freed = offered(&engine, &session, id, coin);
    assert_eq!(
        (freed.listed_free, freed.mixing),
        (false, false),
        "abandoned, the round unseen: the wallet keeps the coin spent: {freed:?}"
    );

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
    assert_eq!(
        status(&engine, &session, id, chunk.txid()),
        Some(TxStatusKind::Abandoned)
    );
    assert!(session.spends.snapshot(&id).is_empty());
}

/// (b), the round's transaction already on the network: the move is
/// refused there and nothing of it is kept. The coin stays offered until
/// the round's block reaches the wallet.
#[test]
fn pin_377_b_a_move_the_network_refuses_leaves_the_coin_offered_until_the_round_is_mined() {
    let (_dir, engine, session, id) = engine_wallet();
    let coin = denomination(&engine, &session, id);
    let round = sign_into_round(&engine, &session, id, coin, |_| {});
    let session = reopen(&engine, session);

    let (moved, sent) = move_mixed(&engine, &session, id, |_| {
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
        EVERYWHERE,
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
