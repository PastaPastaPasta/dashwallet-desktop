//! #170 case 1: the app quits after a payment's sent mark was written but
//! before key-wallet's record of it was stored, whether or not it left. The
//! next start shows the payment pending, keeps the coins it signed out of
//! the balance and coin control, and offers Resend and Abandon.

use std::collections::BTreeSet;
use std::sync::Arc;

use dashcore::{OutPoint, Transaction, Txid};

use super::own_spend_disk_tests::{
    COIN, TIP, abandon, assert_holds, coins, send, spendable, synced, wallet_funded_unmined,
};
use super::{mempool, mined, reopen, rows};
use crate::send::CoinSource;
use crate::session::NetworkSession;
use crate::{Engine, WalletId};

fn held(engine: &Engine, session: &NetworkSession, id: WalletId) -> BTreeSet<OutPoint> {
    coins(engine, session, id)
        .into_iter()
        .map(|c| c.0)
        .collect()
}

fn outputs(tx: &Transaction) -> BTreeSet<OutPoint> {
    (0..tx.output.len() as u32)
        .map(|v| OutPoint::new(tx.txid(), v))
        .collect()
}

fn sent_marks(session: &NetworkSession, id: WalletId) -> BTreeSet<Txid> {
    crate::tx_actions::load_sent(&session.live().unwrap().appdb, id)
        .0
        .into_keys()
        .collect()
}

fn listed(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId, txid: Txid) -> bool {
    rows(engine, session, id)
        .iter()
        .any(|r| r.0 == txid.to_string())
}

/// A payment of A handed to the network, whose record was never stored
/// (the app quit first; dash-spv's own sighting of it never came). Returns
/// it with the session after the restart.
fn sent_unrecorded() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
    Transaction,
) {
    let (dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    let session = reopen(&engine, session);
    (dir, engine, session, id, paid, payment)
}

/// After the restart the payment is listed pending, its mark stays, A is
/// neither counted nor offered, and Abandon and Resend are offered; the
/// same after another restart.
#[test]
fn a_payment_sent_before_a_crash_stays_pending_and_keeps_its_coin_out() {
    let (_dir, engine, session, id, paid, payment) = sent_unrecorded();
    let a = OutPoint::new(paid.txid(), 0);
    let mut want = outputs(&paid);
    want.remove(&a);
    assert!(listed(&engine, &session, id, payment.txid()), "listed");
    assert!(sent_marks(&session, id).contains(&payment.txid()), "marked");
    assert_holds(&engine, &session, id, &want, "restarted");
    assert!(!spendable(&engine, &session, id, a), "A is offered");
    let extras = engine
        .block_on(session.tx_detail_extras(id, payment.txid()))
        .unwrap();
    assert!(extras.can_abandon && extras.can_resend, "{extras:?}");

    let session = reopen(&engine, session);
    assert!(
        listed(&engine, &session, id, payment.txid()),
        "again: listed"
    );
    assert!(sent_marks(&session, id).contains(&payment.txid()));
    assert_holds(&engine, &session, id, &want, "restarted again");
    assert!(!spendable(&engine, &session, id, a), "again: A is offered");
}

/// The payment never left (the app quit just before the hand-off, which
/// the next start cannot tell apart). Abandoning it frees A once the scan
/// passes its funding block, and the mark goes at the next load.
#[test]
fn a_payment_marked_before_a_crash_can_be_abandoned() {
    let (_dir, engine, session, id, paid, payment) = sent_unrecorded();
    let extras = engine
        .block_on(session.tx_detail_extras(id, payment.txid()))
        .unwrap();
    assert!(extras.can_abandon, "{extras:?}");
    abandon(&engine, &session, id, payment.txid());
    assert!(
        (session.hub.history.get(&id, &payment.txid())).is_some_and(|e| e.is_abandoned()),
        "listed abandoned"
    );
    assert_eq!(
        synced(&engine, &session, id),
        super::BIRTH - 1,
        "the abandon rescans the unmined funding from the birth"
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let want = outputs(&paid);
    assert_holds(&engine, &session, id, &want, "abandoned, then scanned");

    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &want, "then a restart");
    assert!(!sent_marks(&session, id).contains(&payment.txid()));
    assert!(
        (session.hub.history.get(&id, &payment.txid())).is_none_or(|e| e.is_abandoned()),
        "not live in the history"
    );
}

/// A peer relays the payment after the restart: it is listed once, A stays
/// out, and its change counts.
#[test]
fn a_payment_sent_before_a_crash_and_relayed_later_counts_once() {
    let (_dir, engine, session, id, paid, payment) = sent_unrecorded();
    let a = OutPoint::new(paid.txid(), 0);
    mempool(&engine, &session, id, &payment);
    let mut want = outputs(&paid);
    want.remove(&a);
    let own = super::own_spend_disk_tests::own_scripts(&engine, &session, id);
    want.insert(super::own_spend_disk_tests::change_of(&payment, &own));
    assert_holds(&engine, &session, id, &want, "relayed");
    let n = (rows(&engine, &session, id).iter())
        .filter(|r| r.0 == payment.txid().to_string())
        .count();
    assert_eq!(n, 1, "listed once");

    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &want, "then a restart");
}

/// The payment is mined after the restart: A stays out, its change counts,
/// and the mark goes at the next load.
#[test]
fn a_payment_sent_before_a_crash_and_mined_later_settles() {
    let (_dir, engine, session, id, paid, payment) = sent_unrecorded();
    let a = OutPoint::new(paid.txid(), 0);
    mined(
        &engine,
        &session,
        id,
        TIP,
        vec![paid.clone(), payment.clone()],
    );
    let mut want = outputs(&paid);
    want.remove(&a);
    let own = super::own_spend_disk_tests::own_scripts(&engine, &session, id);
    want.insert(super::own_spend_disk_tests::change_of(&payment, &own));
    assert_holds(&engine, &session, id, &want, "mined");

    let session = reopen(&engine, session);
    assert_holds(&engine, &session, id, &want, "then a restart");
    assert!(!sent_marks(&session, id).contains(&payment.txid()));
}

/// A mark an earlier build wrote names the coins alone. With no record of
/// its payment it claims nothing and goes at the load, as before.
#[test]
fn an_old_mark_without_its_transaction_and_no_record_goes_at_the_load() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let txid = <Txid as dashcore::hashes::Hash>::from_byte_array([5; 32]);
    let (scope, key) = (id.to_string(), format!("sent:{txid}"));
    (session.live().unwrap().appdb)
        .set_setting(&scope, &key, Some(&a.to_string()))
        .unwrap();
    assert!(
        sent_marks(&session, id).contains(&txid),
        "the old mark reads"
    );

    let session = reopen(&engine, session);
    assert!(held(&engine, &session, id).contains(&a), "A is held");
    assert!(!sent_marks(&session, id).contains(&txid), "the mark went");
}

/// A payment this app sent that a mined spend of its coin by another
/// device beats, paying only someone else: key-wallet sweeps it, and the
/// next launch neither lists it nor keeps its mark.
#[test]
fn a_swept_payment_stays_gone_after_a_restart() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &payment);
    super::own_spend_disk_tests::wait_stored(&session, id, &[payment.txid()]);
    let owner =
        dashcore::Address::from_script(&paid.output[0].script_pubkey, dashcore::Network::Regtest)
            .unwrap();
    let winner = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(a, vec![(super::elsewhere(), COIN - 1_000)]),
    );
    mined(&engine, &session, id, TIP, vec![winner]);
    assert!(!listed(&engine, &session, id, payment.txid()), "swept");
    assert!(
        !sent_marks(&session, id).contains(&payment.txid()),
        "the sweep drops the mark"
    );

    let session = reopen(&engine, session);
    assert!(
        !listed(&engine, &session, id, payment.txid()),
        "listed after a restart"
    );
    assert!(
        !sent_marks(&session, id).contains(&payment.txid()),
        "marked after a restart"
    );
}
