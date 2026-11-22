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

/// A payment marked before a crash whose signed coin's funding the history
/// doesn't hold: abandoning it rescans from the birth height, as nothing
/// says where that coin was funded.
#[test]
fn abandoning_a_marked_payment_of_an_unknown_coin_rescans_from_the_birth() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    mined(&engine, &session, id, TIP, vec![paid]);
    let coin = OutPoint::new(
        <Txid as dashcore::hashes::Hash>::from_byte_array([6; 32]),
        0,
    );
    let payment = super::spend_of(coin, vec![(super::elsewhere(), COIN / 2)]);
    let (scope, txid) = (id.to_string(), payment.txid());
    let hex = dashcore::consensus::encode::serialize_hex(&payment);
    {
        let appdb = &session.live().unwrap().appdb;
        (appdb.set_setting(&scope, &format!("sent:{txid}"), Some(&coin.to_string()))).unwrap();
        (appdb.set_setting(&scope, &format!("senttx:{txid}"), Some(&hex))).unwrap();
    }

    let session = reopen(&engine, session);
    assert!(session.hub.history.get(&id, &txid).is_some(), "restored");
    abandon(&engine, &session, id, txid);
    assert_eq!(
        synced(&engine, &session, id),
        super::BIRTH - 1,
        "the abandon rescans from the birth"
    );
}

/// A coin of the wallet whose funding the store keeps without a record (a
/// height-only row, as for a contact's payment), after a restart.
fn coin_funded_without_a_record(
    engine: &Engine,
    session: Arc<NetworkSession>,
    id: WalletId,
    seed: u8,
) -> (Arc<NetworkSession>, OutPoint) {
    let address = super::own_spend_disk_tests::fresh_address(engine, &session, id);
    let coin = OutPoint::new(
        <Txid as dashcore::hashes::Hash>::from_byte_array([seed; 32]),
        0,
    );
    let txout = dashcore::TxOut {
        value: COIN,
        script_pubkey: address.script_pubkey(),
    };
    let mut utxo = key_wallet::Utxo::new(coin, txout, address, super::BIRTH + 1, false);
    utxo.is_confirmed = true;
    let core = platform_wallet::changeset::CoreChangeSet {
        new_utxos: vec![utxo],
        ..Default::default()
    };
    super::own_spend_disk_tests::store_raw(&session, id, core);
    let session = reopen(engine, session);
    assert!(held(engine, &session, id).contains(&coin), "the coin loads");
    (session, coin)
}

/// A payment of such a coin to someone else, handed off before the app
/// quit, with no change: the next start lists it as a payment, offers
/// Abandon, and the abandon brings the coin back.
#[test]
fn a_payment_of_a_coin_funded_without_a_record_is_listed_and_can_be_abandoned() {
    let (_dir, engine, session, id, _paid) = wallet_funded_unmined();
    let (session, coin) = coin_funded_without_a_record(&engine, session, id, 8);
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![coin]),
        COIN,
        true,
    );
    assert_eq!(payment.output.len(), 1, "no change");

    let session = reopen(&engine, session);
    let row = rows(&engine, &session, id)
        .into_iter()
        .find(|r| r.0 == payment.txid().to_string());
    assert!(row.is_some_and(|r| r.3 < 0), "listed as a payment");
    assert!(
        !held(&engine, &session, id).contains(&coin),
        "its coin is out"
    );
    let extras = engine
        .block_on(session.tx_detail_extras(id, payment.txid()))
        .unwrap();
    assert!(extras.can_abandon && extras.can_resend, "{extras:?}");
    abandon(&engine, &session, id, payment.txid());

    let session = reopen(&engine, session);
    assert!(
        held(&engine, &session, id).contains(&coin),
        "the coin is back"
    );
}

/// As above with change: listed as the payment it is, not as a receipt of
/// its change.
#[test]
fn a_payment_with_change_of_a_coin_funded_without_a_record_is_listed_as_a_payment() {
    let (_dir, engine, session, id, _paid) = wallet_funded_unmined();
    let (session, coin) = coin_funded_without_a_record(&engine, session, id, 9);
    let payment = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![coin]),
        COIN / 2,
        false,
    );
    assert_eq!(payment.output.len(), 2, "with change");

    let session = reopen(&engine, session);
    let row = rows(&engine, &session, id)
        .into_iter()
        .find(|r| r.0 == payment.txid().to_string());
    assert!(
        row.as_ref().is_some_and(|r| r.3 < 0),
        "listed as a payment: {row:?}"
    );
    let extras = engine
        .block_on(session.tx_detail_extras(id, payment.txid()))
        .unwrap();
    assert!(extras.can_abandon && extras.can_resend, "{extras:?}");
}

/// An abandon of the draft while its sent mark is being written is
/// refused: the broadcast goes on, and the mark matches it.
#[test]
fn a_draft_abandoned_while_its_mark_is_written_is_refused() {
    use crate::held_writes::{ReleaseOnDrop, hold_first_job};
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let net = Arc::new(super::own_spend_disk_tests::Accept::default());
    let (draft, prepared) = super::own_spend_disk_tests::prepare(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
        &net,
    );
    let prepared = prepared.unwrap();
    let (pause, task) = hold_first_job(&session, {
        let (draft, prepared) = (Arc::clone(&draft), Arc::clone(&prepared));
        async move {
            draft.broadcast(prepared).await.unwrap();
        }
    });
    let _release = ReleaseOnDrop(Arc::clone(&pause));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !pause.reached.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(std::time::Instant::now() < deadline, "the mark write");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let abandoned = engine.block_on(draft.abandon(Arc::clone(&prepared)));
    assert!(
        matches!(
            abandoned,
            Err(crate::EngineError::Send(
                crate::send::SendFailure::PreparedTxSpent
            ))
        ),
        "{abandoned:?}"
    );
    pause.release.notify_one();
    engine.block_on(task).unwrap();
    let txid: Txid = prepared.summary().txid.parse().unwrap();
    assert_eq!(net.0.lock().unwrap().len(), 1, "handed off");
    assert!(sent_marks(&session, id).contains(&txid), "marked");
}

/// A sent mark that cannot be written leaves the draft pending and no
/// mark: it can still be abandoned.
#[test]
fn a_failed_mark_write_leaves_the_draft_pending_and_no_mark() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let net = Arc::new(super::own_spend_disk_tests::Accept::default());
    let (draft, prepared) = super::own_spend_disk_tests::prepare(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
        &net,
    );
    let prepared = prepared.unwrap();
    let path = session.data_dir().join(dw_appdb::APP_DB_FILE);
    let stage = dw_appdb::fault::Stage::Before;
    dw_appdb::fault::arm(&path, crate::send::SENT_MARK_FAULT, stage);
    assert!(
        engine
            .block_on(draft.broadcast(Arc::clone(&prepared)))
            .is_err()
    );
    dw_appdb::fault::disarm(&path);
    let txid: Txid = prepared.summary().txid.parse().unwrap();
    assert!(net.0.lock().unwrap().is_empty(), "nothing handed off");
    assert!(!sent_marks(&session, id).contains(&txid), "marked");
    engine.block_on(draft.abandon(prepared)).unwrap();
}

/// A marked payment of a coin whose funding the history doesn't hold,
/// relayed back after the restart (its record names no input): abandoning
/// it rescans from the birth height.
#[test]
fn abandoning_a_relayed_marked_payment_of_an_unknown_coin_rescans_from_the_birth() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    mined(&engine, &session, id, TIP, vec![paid]);
    let own = super::own_spend_disk_tests::fresh_address(&engine, &session, id);
    let coin = OutPoint::new(
        <Txid as dashcore::hashes::Hash>::from_byte_array([7; 32]),
        0,
    );
    let payment = super::spend_of(
        coin,
        vec![
            (super::elsewhere(), COIN / 2),
            (own.script_pubkey(), COIN / 4),
        ],
    );
    let (scope, txid) = (id.to_string(), payment.txid());
    let hex = dashcore::consensus::encode::serialize_hex(&payment);
    {
        let appdb = &session.live().unwrap().appdb;
        (appdb.set_setting(&scope, &format!("sent:{txid}"), Some(&coin.to_string()))).unwrap();
        (appdb.set_setting(&scope, &format!("senttx:{txid}"), Some(&hex))).unwrap();
    }

    let session = reopen(&engine, session);
    mempool(&engine, &session, id, &payment);
    abandon(&engine, &session, id, txid);
    assert_eq!(
        synced(&engine, &session, id),
        super::BIRTH - 1,
        "the abandon rescans from the birth"
    );
}

/// One payment's copy that does not decode, and another's that is not
/// text, leave an intact payment restored: each damaged payment's coins
/// stay out, unlisted, with nothing to resend.
#[test]
fn damaged_copies_keep_their_coins_out_and_leave_the_others_restored() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let coin = |v| OutPoint::new(paid.txid(), v);
    let pay = |v| {
        send(
            &engine,
            &session,
            id,
            CoinSource::Outpoints(vec![coin(v)]),
            COIN / 2,
            false,
        )
    };
    let (intact, bad_hex, not_text) = (pay(0), pay(1), pay(2));
    let scope = id.to_string();
    {
        let appdb = &session.live().unwrap().appdb;
        let key = format!("senttx:{}", bad_hex.txid());
        appdb.set_setting(&scope, &key, Some("zz")).unwrap();
    }
    let db = rusqlite::Connection::open(session.data_dir().join(dw_appdb::APP_DB_FILE)).unwrap();
    let key = format!("senttx:{}", not_text.txid());
    db.execute(
        "UPDATE settings_kv SET value = CAST(X'FF' AS TEXT) WHERE scope = ?1 AND key = ?2",
        [&scope, &key],
    )
    .unwrap();
    drop(db);

    let session = reopen(&engine, session);
    assert!(
        listed(&engine, &session, id, intact.txid()),
        "the intact one"
    );
    for (v, tx) in [(0, &intact), (1, &bad_hex), (2, &not_text)] {
        assert!(!spendable(&engine, &session, id, coin(v)), "{v}: offered");
        assert!(
            !held(&engine, &session, id).contains(&coin(v)),
            "{v}: counted"
        );
        if v > 0 {
            assert!(!listed(&engine, &session, id, tx.txid()), "{v}: listed");
        }
    }
}
