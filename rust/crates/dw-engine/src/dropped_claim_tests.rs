//! A payment this app built on the change of a peer's unsigned claim (#739):
//! when the claim is dropped because the scan finds the coin it spends, the
//! payment goes with it, unless it is mined or InstantSend-locked.

use super::own_spend_disk_tests::{change_of, own_scripts, send, spendable};
use super::*;
use crate::send::CoinSource;

/// Mid-sync, a peer's unsigned claim T on a coin the scan hasn't found yet
/// pays [`CHANGE`] to the wallet, and the wallet pays someone from that
/// change (S). Returns the coin's funding, T and S.
fn payment_on_a_claims_change(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
) -> (Transaction, Transaction, Transaction) {
    let (_, funding, claim) = funding_and_spend(engine, session, id);
    mempool(engine, session, id, &claim);
    let change = OutPoint::new(claim.txid(), 1);
    let payment = send(
        engine,
        session,
        id,
        CoinSource::Outpoints(vec![change]),
        CHANGE / 2,
        false,
    );
    mempool(engine, session, id, &payment);
    let ids: Vec<String> = rows(engine, session, id).into_iter().map(|r| r.0).collect();
    assert!(
        ids.contains(&payment.txid().to_string()),
        "the payment is listed"
    );
    (funding, claim, payment)
}

/// The payment is abandoned (or gone), only the coin's funding counts, and
/// the payment's change can't be spent.
fn assert_payment_dropped(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    payment: &Transaction,
) {
    let row = rows(engine, session, id)
        .into_iter()
        .find(|r| r.0 == payment.txid().to_string());
    assert!(
        row.as_ref().is_none_or(|r| r.1 == TxStatusKind::Abandoned),
        "the payment is dropped: {row:?}"
    );
    // The balance is published after the history marks the drop.
    super::own_spend_disk_tests::eventually("only the coin counts", || {
        balances(session, id).total == PAID
    });
    let change = change_of(payment, &own_scripts(engine, session, id));
    assert!(
        !spendable(engine, session, id, change),
        "the payment's change isn't offered"
    );
}

/// The payment is still listed, not abandoned, and its change spendable.
fn assert_payment_kept(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    payment: &Transaction,
) {
    let row = rows(engine, session, id)
        .into_iter()
        .find(|r| r.0 == payment.txid().to_string());
    assert!(
        row.as_ref().is_some_and(|r| r.1 != TxStatusKind::Abandoned),
        "the payment is kept: {row:?}"
    );
    let change = change_of(payment, &own_scripts(engine, session, id));
    assert!(
        spendable(engine, session, id, change),
        "its change is offered"
    );
}

/// After a restart the scan finds the claimed coin, and the next start's
/// store drops the claim as unsigned (#73): the payment from its change is
/// dropped with it, and stays dropped.
#[test]
fn a_payment_on_a_claim_the_store_dropped_goes_with_it() {
    let (_dir, engine, session, id) = engine_wallet();
    let (funding, _claim, payment) = payment_on_a_claims_change(&engine, &session, id);
    let session = reopen(&engine, session);
    skip_unsigned_check(&session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    assert!(
        !dropped(&engine, &session, id, payment.txid()),
        "not dropped in the session"
    );

    let session = reopen(&engine, session);
    assert_payment_dropped(&engine, &session, id, &payment);
    let session = reopen(&engine, session);
    assert_payment_dropped(&engine, &session, id, &payment);
}

/// After a restart a peer relays the claim again, so key-wallet holds it
/// but not the payment (it rebuilds none at load). When the scan finds the
/// coin key-wallet sweeps the claim, and the payment goes with it.
#[test]
fn a_payment_on_a_claim_swept_after_a_restart_goes_with_it() {
    let (_dir, engine, session, id) = engine_wallet();
    let (funding, claim, payment) = payment_on_a_claims_change(&engine, &session, id);
    let session = reopen(&engine, session);
    mempool(&engine, &session, id, &claim);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);

    super::own_spend_disk_tests::eventually("the payment is dropped", || {
        rows(&engine, &session, id)
            .iter()
            .all(|r| r.0 != payment.txid().to_string() || r.1 == TxStatusKind::Abandoned)
    });
    assert_payment_dropped(&engine, &session, id, &payment);
    let session = reopen(&engine, session);
    assert_payment_dropped(&engine, &session, id, &payment);
}

/// The same within one session (#42's live path): key-wallet holds the
/// payment and sweeps it with the claim.
#[test]
fn a_payment_on_a_dropped_claims_change_goes_with_it_in_the_session() {
    let (_dir, engine, session, id) = engine_wallet();
    let (funding, _claim, payment) = payment_on_a_claims_change(&engine, &session, id);

    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    assert_payment_dropped(&engine, &session, id, &payment);
    let session = reopen(&engine, session);
    assert_payment_dropped(&engine, &session, id, &payment);
}

/// A mined payment from the claim's change stays when the store drops the
/// claim.
#[test]
fn a_mined_payment_on_a_dropped_claims_change_is_kept() {
    let (_dir, engine, session, id) = engine_wallet();
    let (funding, _claim, payment) = payment_on_a_claims_change(&engine, &session, id);
    mined(&engine, &session, id, BIRTH - 30, vec![payment.clone()]);
    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);

    let session = reopen(&engine, session);
    assert_payment_kept(&engine, &session, id, &payment);
}

/// An InstantSend-locked payment from the claim's change stays when the
/// store drops the claim.
#[test]
fn a_locked_payment_on_a_dropped_claims_change_is_kept() {
    let (_dir, engine, session, id) = engine_wallet();
    let (funding, _claim, payment) = payment_on_a_claims_change(&engine, &session, id);
    instant_lock(&engine, &session, id, &payment);
    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);

    let session = reopen(&engine, session);
    assert_payment_kept(&engine, &session, id, &payment);
}

/// Whether `txid` is out of the history or abandoned in it.
fn dropped(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId, txid: Txid) -> bool {
    rows(engine, session, id)
        .iter()
        .all(|r| r.0 != txid.to_string() || r.1 == TxStatusKind::Abandoned)
}

/// After a restart key-wallet holds no record of the claim, so it can't
/// drop it when the scan finds the coin: the engine finds the claim
/// unsigned and drops it with the payment from its change, at once. The
/// coin the claim spent waits for the drop's rescan (#818), then counts.
#[test]
fn a_claim_found_unsigned_after_a_restart_goes_with_the_payment_on_its_change() {
    let (_dir, engine, session, id) = engine_wallet();
    let (funding, claim, payment) = payment_on_a_claims_change(&engine, &session, id);
    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding.clone()]);

    super::own_spend_disk_tests::eventually("the claim and the payment are dropped", || {
        dropped(&engine, &session, id, claim.txid())
            && dropped(&engine, &session, id, payment.txid())
    });
    super::own_spend_disk_tests::eventually("the abandon batch is done", || {
        !session.hub.abandon_batch_pending(&id)
    });
    let coin = OutPoint::new(funding.txid(), 0);
    assert_eq!(session.hub.rescan_held(&id), std::collections::BTreeSet::from([coin]));
    assert_eq!(balances(&session, id).total, 0, "the coin waits for the rescan");
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    mined(
        &engine,
        &session,
        id,
        BIRTH + 1,
        vec![super::own_spend_disk_tests::foreign()],
    );
    assert_payment_dropped(&engine, &session, id, &payment);
    let session = reopen(&engine, session);
    assert!(dropped(&engine, &session, id, claim.txid()));
    assert_payment_dropped(&engine, &session, id, &payment);
}

/// A claim signed by the coin's owner with one more push in its input
/// script: the rule can't decide it (`UnsupportedScriptSig`), so after a
/// restart the scan finding the coin doesn't drop it, nor the payment from
/// its change.
#[test]
fn a_claim_the_rule_cant_decide_is_kept_after_a_restart() {
    let (_dir, engine, session, id) = engine_wallet();
    let (to, funding, claim) = funding_and_spend(&engine, &session, id);
    let mut claim = signed_by_wallet(&engine, &session, id, &to, claim);
    let mut script = vec![0x01, 0x01];
    script.extend_from_slice(claim.input[0].script_sig.as_bytes());
    claim.input[0].script_sig = dashcore::ScriptBuf::from_bytes(script);
    let coin = OutPoint::new(funding.txid(), 0);
    assert_eq!(
        key_wallet::transaction_checking::verify_input_authorization(
            &claim,
            0,
            &funding.output[coin.vout as usize],
        ),
        Err(key_wallet::transaction_checking::SpendAuthorizationError::UnsupportedScriptSig)
    );
    mempool(&engine, &session, id, &claim);
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![OutPoint::new(claim.txid(), 1)]),
        CHANGE / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);

    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding.clone()]);
    let s = Arc::clone(&session);
    let manager = session.manager().unwrap();
    let funded = std::collections::BTreeSet::from([funding.txid()]);
    let claims = engine
        .block_on(
            session.on_runtime(async move { Ok(s.unsigned_claims(&manager, id, &funded).await) }),
        )
        .unwrap();
    assert!(claims.is_empty(), "found unsigned: {claims:?}");
    assert!(
        !dropped(&engine, &session, id, claim.txid()),
        "the claim is kept"
    );
    assert!(
        !dropped(&engine, &session, id, payment.txid()),
        "the payment is kept"
    );
}

/// A claim whose input script isn't push-only and has no key of the
/// coin's owner as a push: the rule says unsigned (`Ok(false)`), but a
/// script that runs can build its key and signature, so it is kept.
#[test]
fn a_claim_with_an_input_script_that_isnt_push_only_is_kept_after_a_restart() {
    let (_dir, engine, session, id) = engine_wallet();
    let (_, funding, mut claim) = funding_and_spend(&engine, &session, id);
    claim.input[0].script_sig = dashcore::ScriptBuf::from_bytes(vec![0x61, 0x01, 0x01]);
    assert_eq!(
        key_wallet::transaction_checking::verify_input_authorization(&claim, 0, &funding.output[0]),
        Ok(false)
    );
    mempool(&engine, &session, id, &claim);
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![OutPoint::new(claim.txid(), 1)]),
        CHANGE / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);

    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding.clone()]);
    let s = Arc::clone(&session);
    let manager = session.manager().unwrap();
    let funded = std::collections::BTreeSet::from([funding.txid()]);
    let claims = engine
        .block_on(
            session.on_runtime(async move { Ok(s.unsigned_claims(&manager, id, &funded).await) }),
        )
        .unwrap();
    assert!(claims.is_empty(), "found unsigned: {claims:?}");
    assert!(
        !dropped(&engine, &session, id, claim.txid()),
        "the claim is kept"
    );
    assert!(
        !dropped(&engine, &session, id, payment.txid()),
        "the payment is kept"
    );
    // The store's open now drops the claim on its own verdict: what is built
    // on it stays all the same.
    let session = reopen(&engine, session);
    assert!(
        dropped(&engine, &session, id, claim.txid()),
        "the store dropped the claim"
    );
    assert!(
        !dropped(&engine, &session, id, payment.txid()),
        "the payment is kept after the store drops the claim"
    );
}

/// [`payment_on_a_claims_change`], then the payment settles (`settle`),
/// a restart, and the scan finds the claimed coin: the engine drops
/// neither the claim nor the payment, as something of the claim's tree is
/// settled.
fn kept_under_a_settled_payment(
    settle: impl FnOnce(&Engine, &Arc<NetworkSession>, WalletId, &Transaction),
) {
    let (_dir, engine, session, id) = engine_wallet();
    let (funding, claim, payment) = payment_on_a_claims_change(&engine, &session, id);
    settle(&engine, &session, id, &payment);
    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding.clone()]);

    let s = Arc::clone(&session);
    let manager = session.manager().unwrap();
    let funded = std::collections::BTreeSet::from([funding.txid()]);
    let claims = engine
        .block_on(session.on_runtime(async move {
            let claims = s.unsigned_claims(&manager, id, &funded).await;
            s.abandon_unsigned_claims(&manager, id, claims.clone(), &funded)
                .await;
            Ok(claims)
        }))
        .unwrap();
    assert_eq!(claims, [claim.txid()], "the claim is found unsigned");
    assert!(
        !dropped(&engine, &session, id, claim.txid()),
        "the claim is kept"
    );
    assert_payment_kept(&engine, &session, id, &payment);
}

/// T3: an InstantSend-locked payment on an unsigned claim's change vetoes
/// the drop of the claim and of the payment.
#[test]
fn an_unsigned_claim_under_a_locked_payment_is_kept_after_a_restart() {
    kept_under_a_settled_payment(|engine, session, id, payment| {
        instant_lock(engine, session, id, payment)
    });
}

/// T3: the same for a mined payment.
#[test]
fn an_unsigned_claim_under_a_mined_payment_is_kept_after_a_restart() {
    kept_under_a_settled_payment(|engine, session, id, payment| {
        mined(engine, session, id, BIRTH - 30, vec![payment.clone()])
    });
}

/// The wallet holds coin b, mined, and pays someone from the change of a
/// peer's unsigned claim and b together (S). Returns b's funding, the
/// claimed coin's funding, the claim and S.
fn payment_on_a_claims_change_and_a_coin(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    edit: impl FnOnce(&mut Transaction),
) -> (Transaction, Transaction, Transaction, Transaction) {
    let b_to = super::own_spend_disk_tests::fresh_address(engine, session, id);
    let b_funding = super::own_spend_disk_tests::funding(3, &[b_to], PAID);
    mined(engine, session, id, BIRTH - 40, vec![b_funding.clone()]);
    let (_, funding, mut claim) = funding_and_spend(engine, session, id);
    edit(&mut claim);
    mempool(engine, session, id, &claim);
    let b = OutPoint::new(b_funding.txid(), 0);
    let payment = send(
        engine,
        session,
        id,
        CoinSource::Outpoints(vec![OutPoint::new(claim.txid(), 1), b]),
        CHANGE + PAID / 2,
        false,
    );
    assert_eq!(payment.input.len(), 2, "S spends the change and b");
    mempool(engine, session, id, &payment);
    (b_funding, funding, claim, payment)
}

/// T4 and T5: S also spent b. When the claim is found unsigned after a
/// restart, S goes and b is released: b and the coin the claim spent wait
/// for the drop's rescan (#818), also when b's block comes twice; once the
/// rescan is back b counts once, and after a restart S doesn't come back
/// and b isn't held again.
#[test]
fn a_coin_a_dropped_payment_also_spent_is_offered_again_once() {
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, funding, claim, payment) =
        payment_on_a_claims_change_and_a_coin(&engine, &session, id, |_| {});
    let b = OutPoint::new(b_funding.txid(), 0);
    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    super::own_spend_disk_tests::eventually("the claim and S are dropped", || {
        dropped(&engine, &session, id, claim.txid())
            && dropped(&engine, &session, id, payment.txid())
    });

    for _ in 0..2 {
        mined(&engine, &session, id, BIRTH - 40, vec![b_funding.clone()]);
        assert_eq!(balances(&session, id).total, 0, "they wait for the rescan");
        assert!(!spendable(&engine, &session, id, b), "b waits for the rescan");
    }
    b_scanned_again(&engine, &session, id, &b_funding);

    let session = reopen(&engine, session);
    assert!(
        dropped(&engine, &session, id, payment.txid()),
        "S stays dropped"
    );
    assert_eq!(balances(&session, id).total, 2 * PAID);
    assert!(spendable(&engine, &session, id, b), "b isn't held again");
}

/// T4's variant: another pending transaction of the wallet, which stays,
/// spends b too. S goes, and b stays held by the other.
#[test]
fn a_coin_a_dropped_payment_spent_stays_held_by_another_pending_spend() {
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, funding, claim, payment) =
        payment_on_a_claims_change_and_a_coin(&engine, &session, id, |_| {});
    let b = OutPoint::new(b_funding.txid(), 0);
    let b_owner = Address::from_script(
        &b_funding.output[0].script_pubkey,
        dashcore::Network::Regtest,
    )
    .unwrap();
    let other = signed_by_wallet(
        &engine,
        &session,
        id,
        &b_owner,
        spend_of(
            b,
            vec![(
                super::own_spend_disk_tests::fresh_address(&engine, &session, id).script_pubkey(),
                PAID - 1_000,
            )],
        ),
    );
    mempool(&engine, &session, id, &other);
    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    super::own_spend_disk_tests::eventually("the claim and S are dropped", || {
        dropped(&engine, &session, id, claim.txid())
            && dropped(&engine, &session, id, payment.txid())
    });

    assert!(
        !dropped(&engine, &session, id, other.txid()),
        "the other spend is kept"
    );
    mined(&engine, &session, id, BIRTH - 40, vec![b_funding]);
    assert!(!spendable(&engine, &session, id, b), "b stays held");
    let session = reopen(&engine, session);
    assert!(
        !spendable(&engine, &session, id, b),
        "b stays held after a restart"
    );
}

/// Runs `tx` through the wallet manager as a mempool relay, relevant or not.
fn relay(engine: &Engine, session: &NetworkSession, id: WalletId, tx: &Transaction) {
    let tx = tx.clone();
    deliver(engine, session, id, move |wm| {
        Box::pin(async move {
            wm.process_mempool_transaction(&tx, None).await;
        })
    });
}

/// b's block comes again and the rescan passes the highest checkpoint the
/// wallet had: b counts once with the claimed coin, and is offered.
fn b_scanned_again(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    b_funding: &Transaction,
) {
    // The rescan's blocks come after the drop's rewind (#818).
    super::own_spend_disk_tests::eventually("the abandon batch is done", || {
        !session.hub.abandon_batch_pending(&id)
    });
    mined(engine, session, id, BIRTH - 40, vec![b_funding.clone()]);
    mined(
        engine,
        session,
        id,
        BIRTH + 1,
        vec![super::own_spend_disk_tests::foreign()],
    );
    assert_eq!(balances(session, id).total, 2 * PAID, "b counts once");
    let b = OutPoint::new(b_funding.txid(), 0);
    let (_draft, prepared) = super::own_spend_disk_tests::prepare(
        engine,
        session,
        id,
        CoinSource::Outpoints(vec![b]),
        super::own_spend_disk_tests::COIN / 2,
        false,
        &Arc::new(super::own_spend_disk_tests::Accept::default()),
    );
    assert!(
        prepared
            .as_ref()
            .is_ok_and(|p| p.summary().inputs.iter().any(|i| i.outpoint == b)),
        "b is offered: {:?}",
        prepared.map(|p| p
            .summary()
            .inputs
            .iter()
            .map(|i| i.outpoint)
            .collect::<Vec<_>>())
    );
}

/// T6: a peer relays S again after the engine dropped it with its claim.
/// By the next start at the latest S is dropped again, and b isn't held.
#[test]
fn a_payment_relayed_again_after_its_drop_is_dropped_again() {
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, funding, claim, payment) =
        payment_on_a_claims_change_and_a_coin(&engine, &session, id, |_| {});
    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    super::own_spend_disk_tests::eventually("the claim and S are dropped", || {
        dropped(&engine, &session, id, claim.txid())
            && dropped(&engine, &session, id, payment.txid())
    });
    b_scanned_again(&engine, &session, id, &b_funding);

    relay(&engine, &session, id, &payment);
    let session = reopen(&engine, session);
    assert_payment_dropped_with_b(&engine, &session, id, &payment, &b_funding);
    let session = reopen(&engine, session);
    assert_payment_dropped_with_b(&engine, &session, id, &payment, &b_funding);
}

/// S is dropped, nothing it made counts, and b, scanned again where the
/// drop released it, counts once and is offered.
fn assert_payment_dropped_with_b(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    payment: &Transaction,
    b_funding: &Transaction,
) {
    assert!(dropped(engine, session, id, payment.txid()), "S is dropped");
    b_scanned_again(engine, session, id, b_funding);
    let change = change_of(payment, &own_scripts(engine, session, id));
    assert!(
        !spendable(engine, session, id, change),
        "S's change isn't offered"
    );
}

/// T7: the app stops after the drop's marks are written and before its
/// sweep is stored. The next start ends with S gone and b free.
#[test]
fn a_stop_between_the_drops_marks_and_its_sweep_ends_with_the_payment_gone() {
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, funding, _claim, payment) =
        payment_on_a_claims_change_and_a_coin(&engine, &session, id, |_| {});
    let session = reopen(&engine, session);
    let quit = || {
        session
            .hub
            .quit_before_sweep
            .load(std::sync::atomic::Ordering::SeqCst)
    };
    session
        .hub
        .quit_before_sweep
        .store(true, std::sync::atomic::Ordering::SeqCst);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    super::own_spend_disk_tests::eventually("the drop stops before its sweep", || !quit());

    let session = reopen(&engine, session);
    assert_payment_dropped_with_b(&engine, &session, id, &payment, &b_funding);
    let session = reopen(&engine, session);
    assert_payment_dropped_with_b(&engine, &session, id, &payment, &b_funding);
}

/// T8: a claim whose input script isn't push-only, relayed again after a
/// restart so key-wallet holds it, and swept by key-wallet when the scan
/// finds the coin. Its verdict may be wrong: S is kept and b stays held.
#[test]
fn a_swept_claim_whose_input_script_isnt_push_only_keeps_what_is_built_on_it() {
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, funding, claim, payment) =
        payment_on_a_claims_change_and_a_coin(&engine, &session, id, |claim| {
            claim.input[0].script_sig = dashcore::ScriptBuf::from_bytes(vec![0x61, 0x01, 0x01]);
        });
    let session = reopen(&engine, session);
    relay(&engine, &session, id, &claim);
    assert!(
        session.hub.history.get(&id, &claim.txid()).is_some(),
        "the history holds the claim"
    );
    mined(&engine, &session, id, BIRTH - 20, vec![funding.clone()]);
    assert!(
        session.hub.history.get(&id, &claim.txid()).is_none(),
        "key-wallet swept the claim"
    );
    assert!(
        !dropped_records(&session, id).contains(&claim.txid()),
        "the sweep recorded nothing as dropped"
    );
    assert!(!dropped(&engine, &session, id, payment.txid()), "S is kept");
    let b = OutPoint::new(b_funding.txid(), 0);
    assert!(!spendable(&engine, &session, id, b), "b stays held");
    let session = reopen(&engine, session);
    assert!(
        !dropped(&engine, &session, id, payment.txid()),
        "S is kept after a restart"
    );
    assert!(
        !spendable(&engine, &session, id, b),
        "b stays held after a restart"
    );
}

/// Stops new transactions from starting the engine's check for unsigned
/// claims in `session`, so only the store's open and key-wallet drop them.
fn skip_unsigned_check(session: &Arc<NetworkSession>) {
    (session.hub.skip_unsigned_check).store(true, std::sync::atomic::Ordering::SeqCst);
}

/// The store's open drops the claim, and the load drops S with it: b, which
/// S also spent, is released through the load's abandon and counts once
/// when scanned again.
#[test]
fn a_coin_a_payment_on_a_claim_the_store_dropped_spent_is_offered_again() {
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, funding, claim, payment) =
        payment_on_a_claims_change_and_a_coin(&engine, &session, id, |_| {});
    let session = reopen(&engine, session);
    skip_unsigned_check(&session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    assert!(
        !dropped(&engine, &session, id, claim.txid()),
        "not dropped in the session"
    );

    let session = reopen(&engine, session);
    assert_payment_dropped_with_b(&engine, &session, id, &payment, &b_funding);
    let session = reopen(&engine, session);
    assert_payment_dropped_with_b(&engine, &session, id, &payment, &b_funding);
}

/// The claim pays two change outputs; S1 from one is InstantSend-locked,
/// S2 from the other is pending. Nothing of the claim's tree is dropped:
/// not when the engine finds the claim unsigned, nor when the store drops
/// the claim at the next start.
#[test]
fn a_locked_sibling_keeps_a_pending_payment_on_a_dropped_claim() {
    let (_dir, engine, session, id) = engine_wallet();
    let second = super::own_spend_disk_tests::fresh_address(&engine, &session, id);
    let (_, funding, mut claim) = funding_and_spend(&engine, &session, id);
    claim.output.push(TxOut {
        value: CHANGE,
        script_pubkey: second.script_pubkey(),
    });
    claim.output[0].value -= CHANGE;
    mempool(&engine, &session, id, &claim);
    let from = |vout| {
        send(
            &engine,
            &session,
            id,
            CoinSource::Outpoints(vec![OutPoint::new(claim.txid(), vout)]),
            CHANGE / 2,
            false,
        )
    };
    let (locked, pending) = (from(1), from(2));
    mempool(&engine, &session, id, &locked);
    instant_lock(&engine, &session, id, &locked);
    mempool(&engine, &session, id, &pending);

    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding.clone()]);
    let (s, manager) = (Arc::clone(&session), session.manager().unwrap());
    let funded = std::collections::BTreeSet::from([funding.txid()]);
    engine
        .block_on(session.on_runtime(async move {
            let claims = s.unsigned_claims(&manager, id, &funded).await;
            s.abandon_unsigned_claims(&manager, id, claims, &funded)
                .await;
            Ok(())
        }))
        .unwrap();
    assert!(
        !dropped(&engine, &session, id, claim.txid()),
        "the claim is kept"
    );
    assert!(
        !dropped(&engine, &session, id, pending.txid()),
        "S2 is kept"
    );

    let session = reopen(&engine, session);
    assert!(
        !dropped(&engine, &session, id, pending.txid()),
        "S2 is kept after the store's drop"
    );
    assert!(!dropped(&engine, &session, id, locked.txid()), "S1 is kept");
}

/// S spent b, and after a restart a transaction from another device of the
/// wallet that spends b too is mined. When the claim is found unsigned and
/// S goes, b stays spent; the coin the claim spent counts once the drop's
/// rescan is back (#818).
#[test]
fn a_coin_a_mined_transaction_spends_stays_spent_when_the_payment_goes() {
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, funding, claim, payment) =
        payment_on_a_claims_change_and_a_coin(&engine, &session, id, |_| {});
    let b = OutPoint::new(b_funding.txid(), 0);
    let b_owner = Address::from_script(
        &b_funding.output[0].script_pubkey,
        dashcore::Network::Regtest,
    )
    .unwrap();
    let other = signed_by_wallet(
        &engine,
        &session,
        id,
        &b_owner,
        spend_of(b, vec![(elsewhere(), PAID - 1_000)]),
    );
    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 30, vec![other.clone()]);
    mined(&engine, &session, id, BIRTH - 20, vec![funding.clone()]);
    super::own_spend_disk_tests::eventually("the claim and S are dropped", || {
        dropped(&engine, &session, id, claim.txid())
            && dropped(&engine, &session, id, payment.txid())
    });
    assert!(!spendable(&engine, &session, id, b), "b stays spent");
    assert_eq!(
        balances(&session, id).total,
        0,
        "the claimed coin waits for the rescan"
    );
    mined(&engine, &session, id, BIRTH - 30, vec![other]);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    assert_eq!(
        balances(&session, id).total,
        0,
        "the claimed coin waits until the rescan is back"
    );
    mined(
        &engine,
        &session,
        id,
        BIRTH + 1,
        vec![super::own_spend_disk_tests::foreign()],
    );
    assert!(!spendable(&engine, &session, id, b), "b stays spent, rescanned");
    assert_eq!(balances(&session, id).total, PAID, "b doesn't count");
    let session = reopen(&engine, session);
    assert!(
        !spendable(&engine, &session, id, b),
        "b stays spent after a restart"
    );
    assert_eq!(balances(&session, id).total, PAID);
}

/// The `dropped:` records of `id`.
fn dropped_records(
    session: &Arc<NetworkSession>,
    id: WalletId,
) -> std::collections::BTreeSet<Txid> {
    let (dropped, read) = crate::tx_actions::load_dropped(&session.live().unwrap().appdb, id);
    assert!(read, "the dropped records read");
    dropped.into_keys().collect()
}

/// Path (b) with b: after a restart a peer relays the claim, key-wallet
/// sweeps it when the scan finds the coin, and the drop of S that follows
/// releases b, which counts once when scanned again.
#[test]
fn a_coin_a_payment_on_a_swept_claim_spent_is_offered_again() {
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, funding, claim, payment) =
        payment_on_a_claims_change_and_a_coin(&engine, &session, id, |_| {});
    let session = reopen(&engine, session);
    skip_unsigned_check(&session);
    relay(&engine, &session, id, &claim);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    super::own_spend_disk_tests::eventually("the sweep's drop ran", || {
        (session.hub.dead_hooks_done).load(std::sync::atomic::Ordering::SeqCst) > 0
    });
    assert_payment_dropped_with_b(&engine, &session, id, &payment, &b_funding);
    let session = reopen(&engine, session);
    assert_payment_dropped_with_b(&engine, &session, id, &payment, &b_funding);
}

/// Path (b), stopped right after key-wallet's sweep, before the drop of
/// what is built on the claim ran: the sweep recorded the claim as
/// dropped, and the next start drops S and releases b.
#[test]
fn a_sweep_whose_drop_never_ran_is_finished_at_the_next_start() {
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, funding, claim, payment) =
        payment_on_a_claims_change_and_a_coin(&engine, &session, id, |_| {});
    let session = reopen(&engine, session);
    skip_unsigned_check(&session);
    (session.hub.skip_dead_hook).store(true, std::sync::atomic::Ordering::SeqCst);
    relay(&engine, &session, id, &claim);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    assert!(
        dropped_records(&session, id).contains(&claim.txid()),
        "the sweep recorded the claim"
    );
    assert!(
        !dropped(&engine, &session, id, payment.txid()),
        "S isn't dropped yet"
    );

    let session = reopen(&engine, session);
    assert_payment_dropped_with_b(&engine, &session, id, &payment, &b_funding);
    let session = reopen(&engine, session);
    assert_payment_dropped_with_b(&engine, &session, id, &payment, &b_funding);
}

/// A claim the engine dropped and recorded is mined since (its verdict was
/// wrong): the next start forgets its record.
#[test]
fn a_dropped_claim_mined_since_loses_its_record_at_the_next_start() {
    let (_dir, engine, session, id) = engine_wallet();
    let (funding, claim, _payment) = payment_on_a_claims_change(&engine, &session, id);
    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    super::own_spend_disk_tests::eventually("the claim is dropped", || {
        dropped(&engine, &session, id, claim.txid())
    });
    assert!(dropped_records(&session, id).contains(&claim.txid()));

    mined(&engine, &session, id, BIRTH - 15, vec![claim.clone()]);
    assert!(
        session
            .hub
            .history
            .get(&id, &claim.txid())
            .is_some_and(|e| e.context.block_info().is_some()),
        "the history holds the claim mined"
    );
    assert!(
        dropped_records(&session, id).contains(&claim.txid()),
        "the record stays until the next load"
    );
    let session = reopen(&engine, session);
    assert!(
        !dropped_records(&session, id).contains(&claim.txid()),
        "its record is gone"
    );
}

/// The claim pays two change outputs. S1 from one pays a second loaded
/// wallet B, whose spend of it is InstantSend-locked; S2 from the other is
/// pending. Only B's records show anything of the claim's tree settled:
/// neither the engine's own drop nor the load after the store's drops S2.
#[test]
fn a_settled_spend_in_another_wallet_keeps_a_payment_on_a_dropped_claim() {
    use super::own_spend_disk_tests::{second_wallet, send_to, wait_stored};
    let (_dir, engine, session, id) = engine_wallet();
    let other = second_wallet(&engine, &session);
    engine
        .block_on(session.set_load_on_startup(other, true))
        .unwrap();
    let second = super::own_spend_disk_tests::fresh_address(&engine, &session, id);
    let (_, funding, mut claim) = funding_and_spend(&engine, &session, id);
    claim.output.push(TxOut {
        value: CHANGE,
        script_pubkey: second.script_pubkey(),
    });
    claim.output[0].value -= CHANGE;
    mempool(&engine, &session, id, &claim);
    let to = super::own_spend_disk_tests::fresh_address(&engine, &session, other);
    let s1 = send_to(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![OutPoint::new(claim.txid(), 1)]),
        &to.to_string(),
        CHANGE / 2,
        false,
    );
    mempool(&engine, &session, id, &s1);
    wait_stored(&session, other, &[s1.txid()]);
    let received = (s1.output.iter())
        .position(|o| o.script_pubkey == to.script_pubkey())
        .unwrap() as u32;
    let child = spend_of(
        OutPoint::new(s1.txid(), received),
        vec![
            (elsewhere(), CHANGE / 8),
            (
                super::own_spend_disk_tests::fresh_address(&engine, &session, other)
                    .script_pubkey(),
                CHANGE / 8,
            ),
        ],
    );
    // Delivered with its lock: key-wallet drops an unlocked spend of a held
    // coin whose input isn't signed for it.
    instant_lock(&engine, &session, other, &child);
    wait_stored(&session, other, &[child.txid()]);
    wait_stored_locked(&session, other, child.txid());
    let s2 = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![OutPoint::new(claim.txid(), 2)]),
        CHANGE / 2,
        false,
    );
    mempool(&engine, &session, id, &s2);

    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding.clone()]);
    let (s, manager) = (Arc::clone(&session), session.manager().unwrap());
    let funded = std::collections::BTreeSet::from([funding.txid()]);
    let claims = engine
        .block_on(session.on_runtime(async move {
            let claims = s.unsigned_claims(&manager, id, &funded).await;
            (s.abandon_unsigned_claims(&manager, id, claims.clone(), &funded)).await;
            Ok(claims)
        }))
        .unwrap();
    assert_eq!(claims, [claim.txid()], "the claim is found unsigned");
    assert!(
        !dropped(&engine, &session, id, claim.txid()),
        "the claim is kept"
    );
    assert!(!dropped(&engine, &session, id, s2.txid()), "S2 is kept");

    let session = reopen(&engine, session);
    assert!(
        dropped(&engine, &session, id, claim.txid()),
        "the store dropped the claim"
    );
    assert!(
        !dropped(&engine, &session, id, s2.txid()),
        "S2 is kept after the store's drop"
    );
}

/// A history load whose read of the dropped records fails is incomplete,
/// so it drops nothing; the next load that reads them is complete again.
#[test]
fn a_failed_read_of_the_dropped_records_leaves_the_load_incomplete() {
    let (_dir, engine, session, id) = engine_wallet();
    let load = |session: &Arc<NetworkSession>| {
        let s = Arc::clone(session);
        engine
            .block_on(session.on_runtime(async move { s.load_history_for(vec![id]).await }))
            .unwrap();
        (session.hub.history).read_wallet(&id, |h| h.load_incomplete)
    };
    (session.hub.fail_dropped_read).store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(load(&session), "incomplete while the records don't read");
    assert!(!load(&session), "complete once they read");
}

/// A `dropped:` record whose key names no transaction is left out, and the
/// load stays complete: nothing deletes such a row, and an incomplete load
/// would refuse every Abandon for good.
#[test]
fn a_dropped_record_that_names_no_transaction_is_skipped() {
    let (_dir, engine, session, id) = engine_wallet();
    (session.live().unwrap().appdb)
        .set_setting(&id.to_string(), "dropped:not-a-txid", Some(""))
        .unwrap();
    let s = Arc::clone(&session);
    engine
        .block_on(session.on_runtime(async move { s.load_history_for(vec![id]).await }))
        .unwrap();
    assert!(!(session.hub.history).read_wallet(&id, |h| h.load_incomplete));
    let (dropped, read) = crate::tx_actions::load_dropped(&session.live().unwrap().appdb, id);
    assert!(read && dropped.is_empty(), "{dropped:?}");
}

// ---------------------------------------------------------------------------
// What a dropped payment shows (#790, #766)
// ---------------------------------------------------------------------------

/// The history's records of `txid`.
fn records(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    txid: Txid,
) -> Vec<crate::history::TxRecord> {
    let page = engine
        .block_on(session.history_page(
            id,
            HistoryQuery {
                filter: HistoryFilter::default(),
                sort: HistorySort::NewestFirst,
                cursor: None,
                limit: 100,
            },
        ))
        .unwrap();
    let txid = txid.to_string();
    (page.records.into_iter())
        .filter(|r| r.txid == txid)
        .collect()
}

/// The status of `txid`'s first record, when the history lists it.
fn status(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    txid: Txid,
) -> Option<crate::history::TxStatus> {
    (records(engine, session, id, txid).into_iter())
        .next()
        .map(|r| r.status)
}

/// The category and amount of `txid`'s first record.
fn shown(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    txid: Txid,
) -> Option<(crate::history::TxCategory, i64)> {
    (records(engine, session, id, txid).into_iter())
        .next()
        .map(|r| (r.category, r.amount))
}

/// The drop `txid`'s row shows, if any.
fn drop_of(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    txid: Txid,
) -> Option<crate::history::TxDrop> {
    status(engine, session, id, txid).and_then(|s| s.dropped)
}

/// The txids of the dropped payments' notices.
fn notices(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId) -> Vec<String> {
    let notices = engine.block_on(session.dropped_notices(id)).unwrap();
    notices.into_iter().map(|n| n.txid).collect()
}

/// The transactions `id`'s marks of `prefix` name.
fn marked(
    session: &Arc<NetworkSession>,
    id: WalletId,
    prefix: &str,
) -> std::collections::BTreeSet<Txid> {
    (session.live().unwrap().appdb)
        .settings_with_prefix_lossy(&id.to_string(), prefix)
        .unwrap()
        .into_iter()
        .filter_map(|(k, _)| k[prefix.len()..].parse().ok())
        .collect()
}

/// S, dropped on the claim's change and b, shows Dropped, never counts,
/// offers no Resend, and has no notice until b is back; once b is back it
/// shows Released and its notice, which a dismiss ends for good.
fn assert_dropped_until_b_is_back(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    payment: &Transaction,
    b_funding: &Transaction,
) -> Arc<NetworkSession> {
    use crate::history::{DropCause, DropState};
    let s = payment.txid();
    let drop = drop_of(engine, session, id, s).expect("S is listed dropped");
    assert_eq!(drop.cause, DropCause::InvalidSource);
    assert_eq!(
        status(engine, session, id, s).unwrap().kind,
        TxStatusKind::Abandoned
    );
    let extras = engine.block_on(session.tx_detail_extras(id, s)).unwrap();
    assert!(!extras.can_resend, "a dropped payment offers no Resend");
    assert!(
        matches!(
            engine.block_on(session.resend_transaction(id, s)),
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::AlreadyAbandoned
            ))
        ),
        "and refuses one"
    );
    if drop.state != DropState::Released {
        assert!(
            notices(engine, session, id).is_empty(),
            "no notice before b is back: {drop:?}"
        );
    }
    b_scanned_again(engine, session, id, b_funding);
    let drop = drop_of(engine, session, id, s).unwrap();
    assert_eq!(drop.state, DropState::Released, "b is back");
    assert_eq!(notices(engine, session, id), vec![s.to_string()]);
    let before = shown(engine, session, id, s).unwrap();
    assert_eq!(before.0, crate::history::TxCategory::Sent);
    let session = reopen(engine, Arc::clone(session));
    assert_eq!(
        shown(engine, &session, id, s),
        Some(before),
        "listed as sent after a restart"
    );
    assert_eq!(
        drop_of(engine, &session, id, s).map(|d| (d.cause, d.state)),
        Some((DropCause::InvalidSource, DropState::Released)),
        "after a restart"
    );
    assert_eq!(notices(engine, &session, id), vec![s.to_string()]);
    engine
        .block_on(session.dismiss_dropped_notices(id, vec![s]))
        .unwrap();
    assert!(notices(engine, &session, id).is_empty(), "dismissed");
    let session = reopen(engine, session);
    assert!(
        notices(engine, &session, id).is_empty(),
        "dismissed for good"
    );
    assert_eq!(balances(&session, id).total, 2 * PAID, "S counts nowhere");
    session
}

/// The live path: key-wallet holds the claim and S and sweeps both when
/// the scan finds the claimed coin. S stays listed as dropped, from an
/// abandoned mark written with the sweep's; the claim is gone, with no
/// mark.
#[test]
fn a_payment_swept_with_its_claim_in_the_session_stays_listed_as_dropped() {
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, funding, claim, payment) =
        payment_on_a_claims_change_and_a_coin(&engine, &session, id, |_| {});
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    super::own_spend_disk_tests::eventually("S is listed dropped", || {
        drop_of(&engine, &session, id, payment.txid()).is_some()
    });
    assert!(
        status(&engine, &session, id, claim.txid()).is_none(),
        "the claim isn't listed"
    );
    let abandoned = marked(&session, id, "abandoned:");
    assert!(abandoned.contains(&payment.txid()), "S is marked abandoned");
    assert!(
        !abandoned.contains(&claim.txid()),
        "the claim isn't: {abandoned:?}"
    );
    let session = assert_dropped_until_b_is_back(&engine, &session, id, &payment, &b_funding);
    assert!(
        status(&engine, &session, id, claim.txid()).is_none(),
        "nor after restarts"
    );
}

/// The same payment's abandoned mark, its transaction already swept, does
/// nothing at the next open: no coin changes, no replay is recorded, and
/// the balance stays.
#[test]
fn a_swept_payments_abandoned_mark_changes_nothing_at_the_next_open() {
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, funding, _claim, payment) =
        payment_on_a_claims_change_and_a_coin(&engine, &session, id, |_| {});
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    super::own_spend_disk_tests::eventually("S is listed dropped", || {
        drop_of(&engine, &session, id, payment.txid()).is_some()
    });
    b_scanned_again(&engine, &session, id, &b_funding);
    let coins = |session: &Arc<NetworkSession>| {
        let mut coins: Vec<(OutPoint, u64)> = engine
            .block_on(session.utxos(
                id,
                crate::CoinFilter {
                    include_locked: true,
                    fully_mixed_only: false,
                    min_confirmations: None,
                },
            ))
            .unwrap()
            .into_iter()
            .map(|c| (c.outpoint, c.amount))
            .collect();
        coins.sort();
        coins
    };
    let before = (coins(&session), balances(&session, id).total);
    let session = reopen(&engine, session);
    assert_eq!((coins(&session), balances(&session, id).total), before);
    let replay = engine
        .block_on(session.live().unwrap().replay.get(id))
        .unwrap();
    assert!(replay.is_none(), "no replay recorded: {replay:?}");
}

/// The store drops the claim at an open, and the load drops S.
#[test]
fn a_payment_on_a_claim_the_store_dropped_shows_dropped() {
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, funding, _claim, payment) =
        payment_on_a_claims_change_and_a_coin(&engine, &session, id, |_| {});
    let session = reopen(&engine, session);
    skip_unsigned_check(&session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    let session = reopen(&engine, session);
    assert_dropped_until_b_is_back(&engine, &session, id, &payment, &b_funding);
}

/// After a restart the engine finds the claim unsigned: S shows dropped as
/// built on it, and the claim's own row as the never-valid transaction,
/// which counts nowhere and never has a notice.
#[test]
fn a_claim_found_unsigned_shows_dropped_as_never_valid_and_counts_nowhere() {
    use crate::history::DropCause;
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, funding, claim, payment) =
        payment_on_a_claims_change_and_a_coin(&engine, &session, id, |_| {});
    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    super::own_spend_disk_tests::eventually("the claim and S are dropped", || {
        drop_of(&engine, &session, id, claim.txid()).is_some()
            && drop_of(&engine, &session, id, payment.txid()).is_some()
    });
    let claim_row = |session: &Arc<NetworkSession>| records(&engine, session, id, claim.txid());
    let rows = claim_row(&session);
    assert!(!rows.is_empty(), "the claim is listed");
    for r in rows {
        assert_eq!(
            r.status.dropped.map(|d| d.cause),
            Some(DropCause::InvalidClaim)
        );
        assert!(!r.counts_toward_balance, "the claim counts nowhere");
    }
    let session = assert_dropped_until_b_is_back(&engine, &session, id, &payment, &b_funding);
    let b = balances(&session, id);
    assert_eq!(
        (b.total, b.unconfirmed, b.locked),
        (2 * PAID, 0, 0),
        "not in the balance, pending or held"
    );
    let rows = claim_row(&session);
    assert!(!rows.is_empty(), "the claim is listed after restarts");
    for r in rows {
        assert_eq!(
            r.status.dropped.map(|d| d.cause),
            Some(DropCause::InvalidClaim),
            "after restarts"
        );
    }
}

/// A payment the user abandons shows no drop.
#[test]
fn a_payment_the_user_abandons_shows_no_drop() {
    let (_dir, engine, session, id) = engine_wallet();
    let (_funding, _claim, payment) = payment_on_a_claims_change(&engine, &session, id);
    super::own_spend_disk_tests::start_offline_spv(&engine, &session);
    engine
        .block_on(session.abandon_transaction(id, payment.txid()))
        .unwrap();
    let st = status(&engine, &session, id, payment.txid()).unwrap();
    assert_eq!((st.kind, st.dropped), (TxStatusKind::Abandoned, None));
    let session = reopen(&engine, session);
    let st = status(&engine, &session, id, payment.txid()).unwrap();
    assert_eq!((st.kind, st.dropped), (TxStatusKind::Abandoned, None));
    assert!(notices(&engine, &session, id).is_empty());
}

/// A drop an earlier build recorded, its mark's value empty, reads as built
/// on a never-valid claim.
#[test]
fn an_earlier_builds_dropped_mark_reads_as_built_on_a_claim() {
    use crate::history::DropCause;
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, funding, _claim, payment) =
        payment_on_a_claims_change_and_a_coin(&engine, &session, id, |_| {});
    let session = reopen(&engine, session);
    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    super::own_spend_disk_tests::eventually("S is listed dropped", || {
        drop_of(&engine, &session, id, payment.txid()).is_some()
    });
    b_scanned_again(&engine, &session, id, &b_funding);
    (session.live().unwrap().appdb)
        .set_setting(
            &id.to_string(),
            &format!("dropped:{}", payment.txid()),
            Some(""),
        )
        .unwrap();
    let session = reopen(&engine, session);
    assert_eq!(
        drop_of(&engine, &session, id, payment.txid()).map(|d| d.cause),
        Some(DropCause::InvalidSource)
    );
}

/// S dropped when the scan finds the claimed coin after a restart, with
/// the rewind that brings b back, before b is back. `fail_rewind` makes
/// that rewind fail after its record is stored.
fn dropped_after_a_restart(
    engine: &Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    fail_rewind: bool,
) -> (Arc<NetworkSession>, Transaction, Transaction) {
    let (b_funding, funding, _claim, payment) =
        payment_on_a_claims_change_and_a_coin(engine, &session, id, |_| {});
    let session = reopen(engine, session);
    if fail_rewind {
        (session.live().unwrap().replay)
            .crash_after_write
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    mined(engine, &session, id, BIRTH - 20, vec![funding]);
    super::own_spend_disk_tests::eventually("S is listed dropped", || {
        drop_of(engine, &session, id, payment.txid()).is_some()
    });
    (session, b_funding, payment)
}

/// The state S's row shows once its abandon batch is done.
fn settled_state(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    s: Txid,
) -> crate::history::DropState {
    super::own_spend_disk_tests::eventually("the abandon batch is done", || {
        !session.hub.abandon_batch_pending(&id)
    });
    drop_of(engine, session, id, s)
        .expect("S is listed dropped")
        .state
}

/// The drop's rewind runs: S shows Releasing until the rescan passes b's
/// block, then Released.
#[test]
fn a_dropped_payment_shows_releasing_while_its_rescan_runs() {
    use crate::history::DropState;
    let (_dir, engine, session, id) = engine_wallet();
    let (session, b_funding, payment) = dropped_after_a_restart(&engine, session, id, false);
    let state = settled_state(&engine, &session, id, payment.txid());
    assert_eq!(state, DropState::Releasing);
    assert!(notices(&engine, &session, id).is_empty());
    b_scanned_again(&engine, &session, id, &b_funding);
    assert_eq!(
        drop_of(&engine, &session, id, payment.txid())
            .unwrap()
            .state,
        DropState::Released
    );
}

/// The drop's rewind fails: no scan brings b back, so S asks for one,
/// here from below the birth height b's block is under. b's block coming
/// with no rewind brings nothing back: b waits for a rescan (#818). The
/// rescan a lower birth height starts brings b back and makes S Released.
#[test]
fn a_dropped_payment_whose_rewind_failed_asks_for_a_rescan() {
    use crate::history::DropState;
    let (_dir, engine, session, id) = engine_wallet();
    let (session, b_funding, payment) = dropped_after_a_restart(&engine, session, id, true);
    let state = settled_state(&engine, &session, id, payment.txid());
    assert_eq!(state, DropState::RescanFromHeight { height: BIRTH - 40 });
    assert!(notices(&engine, &session, id).is_empty());
    mined(&engine, &session, id, BIRTH - 40, vec![b_funding.clone()]);
    assert_eq!(balances(&session, id).total, 0, "no rescan ran");
    assert_eq!(
        drop_of(&engine, &session, id, payment.txid())
            .unwrap()
            .state,
        DropState::RescanFromHeight { height: BIRTH - 40 }
    );
    super::own_spend_disk_tests::start_offline_spv(&engine, &session);
    engine
        .block_on(session.set_birth_height(id, BIRTH - 50))
        .unwrap();
    b_scanned_again(&engine, &session, id, &b_funding);
    assert_eq!(
        drop_of(&engine, &session, id, payment.txid())
            .unwrap()
            .state,
        DropState::Released
    );
    assert_eq!(
        notices(&engine, &session, id),
        vec![payment.txid().to_string()]
    );
}

/// Saving a birth height at or below b's block starts a rescan: S shows
/// Releasing, then Released once it brings b back.
#[test]
fn a_lower_birth_height_rescans_and_brings_a_dropped_payments_coin_back() {
    use crate::history::DropState;
    let (_dir, engine, session, id) = engine_wallet();
    let (session, b_funding, payment) = dropped_after_a_restart(&engine, session, id, true);
    assert_eq!(
        settled_state(&engine, &session, id, payment.txid()),
        DropState::RescanFromHeight { height: BIRTH - 40 }
    );
    super::own_spend_disk_tests::start_offline_spv(&engine, &session);
    engine
        .block_on(session.set_birth_height(id, BIRTH - 50))
        .unwrap();
    let state = drop_of(&engine, &session, id, payment.txid())
        .unwrap()
        .state;
    assert_eq!(state, DropState::Releasing);
    b_scanned_again(&engine, &session, id, &b_funding);
    assert_eq!(
        drop_of(&engine, &session, id, payment.txid())
            .unwrap()
            .state,
        DropState::Released
    );
}

/// The rescan a lower birth started is cancelled before b's block: S asks
/// for a rescan again, now from the birth, which b is above; never
/// Releasing without a scan running.
#[test]
fn a_cancelled_rescan_leaves_a_dropped_payment_asking_for_one() {
    use crate::history::DropState;
    let (_dir, engine, session, id) = engine_wallet();
    let (session, b_funding, payment) = dropped_after_a_restart(&engine, session, id, true);
    super::own_spend_disk_tests::start_offline_spv(&engine, &session);
    engine
        .block_on(session.set_birth_height(id, BIRTH - 50))
        .unwrap();
    assert_eq!(
        drop_of(&engine, &session, id, payment.txid())
            .unwrap()
            .state,
        DropState::Releasing
    );
    assert!(engine.block_on(session.cancel_rescan()).unwrap());
    let state = drop_of(&engine, &session, id, payment.txid())
        .unwrap()
        .state;
    assert_eq!(state, DropState::RescanNeeded);
    // A later block brings b back to key-wallet, but b waits for the
    // rescan the cancel left owed (#818).
    mined(&engine, &session, id, BIRTH - 40, vec![b_funding]);
    assert_eq!(balances(&session, id).total, 0, "they wait for the rescan");
    assert_eq!(
        drop_of(&engine, &session, id, payment.txid())
            .unwrap()
            .state,
        DropState::RescanNeeded
    );
}

/// b's funding is still unmined when S is dropped: S waits for it, and
/// shows Released by itself once its block comes.
#[test]
fn a_dropped_payment_on_an_unmined_coin_is_released_when_it_confirms() {
    use crate::history::DropState;
    let (_dir, engine, session, id) = engine_wallet();
    let b_to = super::own_spend_disk_tests::fresh_address(&engine, &session, id);
    let b_funding = super::own_spend_disk_tests::funding(3, &[b_to], PAID);
    mempool(&engine, &session, id, &b_funding);
    let (_, funding, claim) = funding_and_spend(&engine, &session, id);
    mempool(&engine, &session, id, &claim);
    let b = OutPoint::new(b_funding.txid(), 0);
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![OutPoint::new(claim.txid(), 1), b]),
        CHANGE + PAID / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);
    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    super::own_spend_disk_tests::eventually("S is listed dropped", || {
        drop_of(&engine, &session, id, payment.txid()).is_some()
    });
    let state = settled_state(&engine, &session, id, payment.txid());
    assert_eq!(state, DropState::AwaitingFunding);
    mined(&engine, &session, id, BIRTH + 1, vec![b_funding]);
    let state = drop_of(&engine, &session, id, payment.txid())
        .unwrap()
        .state;
    assert_eq!(state, DropState::Released);
}

/// The app stops after an in-session sweep's marks are written (the claim
/// and S dropped, S's abandoned mark) and before key-wallet's sweep is
/// stored: the store still holds both. The next start ends as the other
/// drops do: S gone from the balance, listed as dropped, and b back once
/// scanned again.
#[test]
fn a_stop_after_an_in_session_sweeps_marks_ends_with_the_payment_dropped() {
    use crate::history::{DropCause, DropState};
    let (_dir, engine, session, id) = engine_wallet();
    let (b_funding, funding, claim, payment) =
        payment_on_a_claims_change_and_a_coin(&engine, &session, id, |_| {});
    let (t, s) = (claim.txid(), payment.txid());
    let own = session.hub.history.get(&id, &s).unwrap().own_inputs;
    let dead = crate::tx_actions::Dead::from([
        (t, DropCause::InvalidClaim),
        (s, DropCause::InvalidSource),
    ]);
    crate::tx_actions::forget_swept(
        &session.live().unwrap().appdb,
        id,
        &[t, s],
        &dead,
        &[],
        &[(s, payment.clone(), own)],
    )
    .unwrap();
    let session = reopen(&engine, session);
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);
    super::own_spend_disk_tests::eventually("the claim and S are dropped", || {
        dropped(&engine, &session, id, t) && dropped(&engine, &session, id, s)
    });
    assert_eq!(
        drop_of(&engine, &session, id, s).map(|d| d.cause),
        Some(DropCause::InvalidSource)
    );
    let session = reopen(&engine, session);
    assert_payment_dropped_with_b(&engine, &session, id, &payment, &b_funding);
    assert_eq!(
        drop_of(&engine, &session, id, s).map(|d| (d.cause, d.state)),
        Some((DropCause::InvalidSource, DropState::Released))
    );
}
