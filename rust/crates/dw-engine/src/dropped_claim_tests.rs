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
    assert_eq!(
        balances(session, id).total,
        PAID,
        "nothing the payment made counts"
    );
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
    mined(&engine, &session, id, BIRTH - 20, vec![funding]);

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
