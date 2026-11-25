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
use super::{instant_lock, mempool, mined, reopen, rows, wait_stored_locked};
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

/// A payment restored from its mark after a restart, which the next start
/// can't tell from one that never left (the app quit just before the
/// hand-off); the fixture's was handed off. The quit before the hand-off is
/// `h03_kill9_core_tests::h03_core_send_killed_before_the_hand_off_keeps_its_coins_spent`.
/// Abandoning it frees A once the scan passes its funding block. Its mark
/// stays (an abandoned payment's does until it is ChainLocked) and claims
/// nothing.
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
    assert!(
        sent_marks(&session, id).contains(&payment.txid()),
        "the mark went"
    );
    assert!(
        (session.hub.history.get(&id, &payment.txid())).is_none_or(|e| e.is_abandoned()),
        "not live in the history"
    );
}

/// Coverage: the payment, listed pending after the restart, is relayed
/// InstantSend-locked and stays unmined. Its mark stays, so after another
/// restart its coin stays out even when its funding block is scanned again.
#[test]
fn a_payment_restored_after_a_crash_and_then_locked_keeps_its_coin_out() {
    let (_dir, engine, session, id, paid, payment) = sent_unrecorded();
    let a = OutPoint::new(paid.txid(), 0);
    assert!(listed(&engine, &session, id, payment.txid()), "listed");
    mempool(&engine, &session, id, &payment);
    instant_lock(&engine, &session, id, &payment);
    wait_stored_locked(&session, id, payment.txid());

    let session = reopen(&engine, session);
    assert!(sent_marks(&session, id).contains(&payment.txid()), "marked");
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(!held(&engine, &session, id).contains(&a), "A is credited");
    assert!(!spendable(&engine, &session, id, a), "A is offered");
    assert!(
        (session.hub.history.get(&id, &payment.txid())).is_some_and(|e| !e.is_abandoned()),
        "listed"
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

/// The payment is mined after the restart: A stays out and its change
/// counts. Its mark stays until the payment is ChainLocked, and claims
/// nothing meanwhile.
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
    assert!(
        sent_marks(&session, id).contains(&payment.txid()),
        "the mark went"
    );
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
    // Not [7; 32], whose output 0 the test wallet's funding spends.
    let coin = OutPoint::new(
        <Txid as dashcore::hashes::Hash>::from_byte_array([0x5c; 32]),
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

/// Tools ▸ Repair ▸ Remove unconfirmed transactions retires a payment
/// whose saved copy is damaged: its coin comes back, also after a restart.
/// A readable mark stays, and a coin another payment spends stays out.
#[test]
fn remove_unconfirmed_retires_a_damaged_payment_and_frees_its_coin() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let kept = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![b]),
        COIN / 2,
        false,
    );
    let damaged = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &kept);
    super::instant_lock(&engine, &session, id, &kept);
    {
        let (appdb, scope) = (&session.live().unwrap().appdb, id.to_string());
        let txid = damaged.txid();
        appdb
            .set_setting(&scope, &format!("senttx:{txid}"), Some("zz"))
            .unwrap();
        appdb
            .set_setting(&scope, &format!("sent:{txid}"), Some(&format!("{a},{b}")))
            .unwrap();
    }
    let session = reopen(&engine, session);
    assert!(!held(&engine, &session, id).contains(&a), "A is out");
    assert!(!held(&engine, &session, id).contains(&b), "B is out");

    super::own_spend_disk_tests::start_offline_spv(&engine, &session);
    let dropped = engine.block_on(session.drop_unconfirmed(Some(id))).unwrap();
    assert_eq!(dropped, 1, "the damaged payment is counted");
    assert!(
        !sent_marks(&session, id).contains(&damaged.txid()),
        "retired"
    );
    assert!(
        sent_marks(&session, id).contains(&kept.txid()),
        "the readable mark stays"
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(held(&engine, &session, id).contains(&a), "A is counted");
    assert!(spendable(&engine, &session, id, a), "A is offered");
    assert!(!spendable(&engine, &session, id, b), "B is offered");

    let session = reopen(&engine, session);
    assert!(
        held(&engine, &session, id).contains(&a),
        "A after a restart"
    );
    assert!(
        spendable(&engine, &session, id, a),
        "A offered after a restart"
    );
    assert!(
        !held(&engine, &session, id).contains(&b),
        "B after a restart"
    );
}

/// As above, but another record of the wallet doesn't read at its load:
/// what descends from the damaged payment is unknown, so Remove
/// Unconfirmed keeps its mark and its coin out, and fails with
/// `RecordUnreadable` naming the wallet (#611).
#[test]
fn remove_unconfirmed_keeps_a_damaged_payment_while_a_record_does_not_read() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let kept = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![b]),
        COIN / 2,
        false,
    );
    let damaged = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &kept);
    super::instant_lock(&engine, &session, id, &kept);
    wait_stored_locked(&session, id, kept.txid());
    {
        let (appdb, scope) = (&session.live().unwrap().appdb, id.to_string());
        let txid = damaged.txid();
        appdb
            .set_setting(&scope, &format!("senttx:{txid}"), Some("zz"))
            .unwrap();
        appdb
            .set_setting(&scope, &format!("sent:{txid}"), Some(&format!("{a},{b}")))
            .unwrap();
    }
    let session = reopen(&engine, session);
    engine.block_on(session.unload_wallet(id)).unwrap();
    session.hub.unreadable_record.lock().unwrap().0 = Some(kept.txid());
    engine.block_on(session.load_wallet(id)).unwrap();

    super::own_spend_disk_tests::start_offline_spv(&engine, &session);
    let dropped = engine.block_on(session.drop_unconfirmed(Some(id)));
    assert!(
        matches!(
            dropped,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::RecordUnreadable(w)
            )) if w == id
        ),
        "{dropped:?}"
    );
    assert!(
        sent_marks(&session, id).contains(&damaged.txid()),
        "the damaged payment's mark stays"
    );
    assert!(!held(&engine, &session, id).contains(&a), "A is counted");
    assert!(!spendable(&engine, &session, id, a), "A is offered");
}

/// A restored payment of a coin funded without a record, relayed back
/// (its record names no input), in the next session: listed as the
/// payment it is, and abandoning it rescans from the birth height.
#[test]
fn a_relayed_restored_payment_keeps_its_inputs_in_the_next_session() {
    let (_dir, engine, session, id, _paid) = wallet_funded_unmined();
    let (session, coin) = coin_funded_without_a_record(&engine, session, id, 10);
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
    mempool(&engine, &session, id, &payment);
    super::own_spend_disk_tests::wait_stored(&session, id, &[payment.txid()]);

    let session = reopen(&engine, session);
    let row = rows(&engine, &session, id)
        .into_iter()
        .find(|r| r.0 == payment.txid().to_string());
    assert!(
        row.as_ref().is_some_and(|r| r.3 < 0),
        "listed as a payment: {row:?}"
    );
    abandon(&engine, &session, id, payment.txid());
    assert_eq!(
        synced(&engine, &session, id),
        super::BIRTH - 1,
        "the abandon rescans from the birth"
    );
}

/// A sent mark whose first write and drop both fail: the payment is held
/// for the session with the storage error (its mark is most likely not
/// stored), and neither a retry nor an Abandon of the draft hands it off
/// or frees its coin.
#[test]
fn a_mark_that_cannot_be_written_or_dropped_refuses_a_retry_with_the_storage_error() {
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
    dw_appdb::fault::arm(&path, crate::send::SENT_MARK_FAULT, stage);
    let first = engine.block_on(draft.broadcast(Arc::clone(&prepared)));
    assert!(
        matches!(first, Err(crate::EngineError::Storage(_))),
        "{first:?}"
    );
    dw_appdb::fault::disarm(&path);
    let again = engine.block_on(draft.broadcast(Arc::clone(&prepared)));
    assert!(
        matches!(again, Err(crate::EngineError::Storage(_))),
        "{again:?}"
    );
    assert!(net.0.lock().unwrap().is_empty(), "a retry handed it off");
    let abandoned = engine.block_on(draft.abandon(prepared));
    assert!(
        matches!(abandoned, Err(crate::EngineError::Storage(_))),
        "{abandoned:?}"
    );
    assert!(!spendable(&engine, &session, id, a), "its coin is offered");
}

/// A payment that never left whose sent mark cannot be dropped stays held
/// as maybe sent: Abandon is refused and its coin stays out, so the copy
/// left behind cannot be resent for coins already freed.
#[test]
fn a_mark_that_cannot_be_dropped_keeps_the_payment_held() {
    struct RefuseAndBreakTheMarks(std::path::PathBuf);
    impl crate::send::TestBroadcaster for RefuseAndBreakTheMarks {
        fn before_entry(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
            Box::pin(async {})
        }
        fn broadcast(
            &self,
            _tx: &Transaction,
            _first: bool,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<Txid, platform_wallet::PlatformWalletError>>
                    + Send,
            >,
        > {
            let stage = dw_appdb::fault::Stage::Before;
            dw_appdb::fault::arm(&self.0, crate::send::SENT_MARK_FAULT, stage);
            Box::pin(async {
                Err(platform_wallet::PlatformWalletError::TransactionBroadcast(
                    "rejected".into(),
                ))
            })
        }
    }
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
    *draft.broadcaster.lock().unwrap() = Some(Arc::new(RefuseAndBreakTheMarks(path.clone())) as _);
    let sent = engine.block_on(draft.broadcast(Arc::clone(&prepared)));
    assert!(
        matches!(
            sent,
            Err(crate::EngineError::Send(
                crate::send::SendFailure::HeldUntilRestart
            ))
        ),
        "{sent:?}"
    );
    dw_appdb::fault::disarm(&path);
    let txid: Txid = prepared.summary().txid.parse().unwrap();
    assert!(sent_marks(&session, id).contains(&txid), "the mark stays");
    *draft.broadcaster.lock().unwrap() = Some(Arc::clone(&net) as _);
    let again = engine.block_on(draft.broadcast(Arc::clone(&prepared)));
    assert!(
        matches!(
            again,
            Err(crate::EngineError::Send(
                crate::send::SendFailure::HeldUntilRestart
            ))
        ),
        "{again:?}"
    );
    assert!(net.0.lock().unwrap().is_empty(), "a retry handed it off");
    let abandoned = engine.block_on(draft.abandon(prepared));
    assert!(
        matches!(
            abandoned,
            Err(crate::EngineError::Send(
                crate::send::SendFailure::HeldUntilRestart
            ))
        ),
        "{abandoned:?}"
    );
    assert!(!spendable(&engine, &session, id, a), "its coin is offered");
    drop(draft);

    // The way out: after a restart the payment is listed pending from its
    // copy, and its Abandon frees A.
    let session = reopen(&engine, session);
    assert!(
        listed(&engine, &session, id, txid),
        "listed after the restart"
    );
    let extras = engine.block_on(session.tx_detail_extras(id, txid)).unwrap();
    assert!(extras.can_abandon, "{extras:?}");
    abandon(&engine, &session, id, txid);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert_holds(
        &engine,
        &session,
        id,
        &outputs(&paid),
        "abandoned, then scanned",
    );
}

/// The app stops after an Abandon's sweep removed the payment's stored
/// record: the next start keeps it abandoned, not pending from its saved
/// copy, and A comes back once its funding is scanned.
#[test]
fn an_abandon_stopped_after_its_sweep_stays_abandoned_after_a_restart() {
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
    assert!(sent_marks(&session, id).contains(&payment.txid()), "marked");

    crate::tx_actions::sweep_stop::arm(&*session.live().unwrap().persister);
    let (s, txid) = (Arc::clone(&session), payment.txid());
    let stopped = engine.block_on(session.on_runtime(async move {
        let manager = s.manager()?;
        s.abandon_inner(&manager, id, txid).await
    }));
    assert!(stopped.is_err(), "stopped after the sweep: {stopped:?}");
    assert!(
        !crate::tx_actions::sweep_stop::armed(&*session.live().unwrap().persister),
        "the stop was hit"
    );

    let session = reopen(&engine, session);
    assert!(
        (session.hub.history.get(&id, &txid)).is_none_or(|e| e.is_abandoned()),
        "restored as pending"
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert_holds(&engine, &session, id, &outputs(&paid), "then scanned");
}

/// The store's open dropped a pending spend whose input isn't signed by its
/// coin's owner and gave the coin back: a sent mark of that spend neither
/// lists it nor keeps the coin out after the restart.
#[test]
#[ignore = "Q112 port (Q-R9 C2): spec for the desktop pre-open repair of unsigned pending spends"]
fn a_spend_the_store_dropped_as_unsigned_is_not_restored_from_its_mark() {
    use key_wallet::account::{AccountType, StandardAccountType};
    use key_wallet::managed_account::transaction_record::{
        TransactionDirection, TransactionRecord,
    };
    use key_wallet::transaction_checking::TransactionContext;
    use key_wallet::transaction_checking::transaction_router::TransactionType;
    use platform_wallet::changeset::{
        CoreChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
    };

    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let claim = Transaction {
        version: 3,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: a,
            script_sig: dashcore::ScriptBuf::from_bytes(vec![0x4c, 0xff, 0x01]),
            sequence: u32::MAX,
            witness: Default::default(),
        }],
        output: vec![dashcore::TxOut {
            value: COIN / 2,
            script_pubkey: dashcore::ScriptBuf::from_bytes(vec![0x6a]),
        }],
        special_transaction_payload: None,
    };
    let record = TransactionRecord::new(
        claim.clone(),
        AccountType::Standard {
            index: 0,
            standard_account_type: StandardAccountType::BIP44Account,
        },
        TransactionContext::Mempool,
        TransactionType::Standard,
        TransactionDirection::Internal,
        Vec::new(),
        Vec::new(),
        0,
    );
    let live = session.live().unwrap();
    let changeset = PlatformWalletChangeSet {
        core: Some(CoreChangeSet {
            records: vec![record],
            ..Default::default()
        }),
        ..Default::default()
    };
    live.persister.store(id.0, changeset).unwrap();
    live.persister.flush(id.0).unwrap();
    let (scope, txid) = (id.to_string(), claim.txid());
    let mark = |prefix: &str, value: String| {
        (live.appdb)
            .set_setting(&scope, &format!("{prefix}{txid}"), Some(&value))
            .unwrap();
    };
    mark(crate::tx_actions::SENT_PREFIX, a.to_string());
    mark(
        crate::tx_actions::SENT_TX_PREFIX,
        dashcore::consensus::encode::serialize_hex(&claim),
    );
    drop(live);

    let session = reopen(&engine, session);
    assert!(
        !listed(&engine, &session, id, txid),
        "restored from its mark"
    );
    assert!(spendable(&engine, &session, id, a), "A is held");
}

/// A child of a pending payment's change that key-wallet recorded while its
/// event was still queued for the history: the wallet, the funding, the
/// parent, the child and the queued events.
fn child_recorded_before_its_event() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
    Transaction,
    Transaction,
    Vec<platform_wallet::key_wallet_manager::WalletEvent>,
) {
    use platform_wallet::key_wallet_manager::WalletInterface;
    let (dir, engine, session, id, paid) = wallet_funded_unmined();
    let (a, b) = (OutPoint::new(paid.txid(), 0), OutPoint::new(paid.txid(), 1));
    let parent = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &parent);
    let own = super::own_spend_disk_tests::own_scripts(&engine, &session, id);
    let change = super::own_spend_disk_tests::change_of(&parent, &own);
    let child = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![change, b]),
        COIN,
        false,
    );
    let manager = session.manager().unwrap();
    let recorded = child.clone();
    let queued = engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let mut rx = wm.subscribe_events();
        assert!(
            wm.process_mempool_transaction(&recorded, None)
                .await
                .is_relevant
        );
        std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>()
    });
    assert!(
        session.hub.history.get(&id, &child.txid()).is_none(),
        "the history has the child"
    );
    (dir, engine, session, id, paid, parent, child, queued)
}

/// key-wallet can record a payment before its event reaches the history.
/// An Abandon of its parent marks it abandoned too, so after a restart it
/// isn't restored pending from its saved copy, and its other coin is free
/// once its funding is scanned.
#[test]
fn an_abandon_marks_a_child_key_wallet_recorded_before_the_history_did() {
    let (_dir, engine, session, id, paid, parent, child, _queued) =
        child_recorded_before_its_event();
    let b = OutPoint::new(paid.txid(), 1);
    abandon(&engine, &session, id, parent.txid());

    let session = reopen(&engine, session);
    assert!(
        (session.hub.history.get(&id, &child.txid())).is_none_or(|e| e.is_abandoned()),
        "the child is restored pending"
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(spendable(&engine, &session, id, b), "B stays held");
}

/// What an abandoned child must read like: abandoned, not resent, and none
/// of its outputs offered.
fn assert_child_gone(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    child: &Transaction,
    state: &str,
) {
    assert!(
        (session.hub.history.get(&id, &child.txid())).is_none_or(|e| e.is_abandoned()),
        "{state}: the child reads pending"
    );
    let extras = engine
        .block_on(session.tx_detail_extras(id, child.txid()))
        .ok();
    assert!(
        extras.as_ref().is_none_or(|x| !x.can_resend),
        "{state}: {extras:?}"
    );
    let resent = engine.block_on(session.resend_transaction(id, child.txid()));
    assert!(
        matches!(
            resent,
            Err(crate::EngineError::TxActionRefused(
                crate::tx_actions::TxActionRefusal::AlreadyAbandoned
            ))
        ),
        "{state}: {resent:?}"
    );
    for o in outputs(child) {
        assert!(
            !spendable(engine, session, id, o),
            "{state}: {o} is offered"
        );
    }
}

/// The child's event, queued before the Abandon and delivered after it,
/// doesn't bring it back: it reads abandoned and can't be resent, its
/// outputs are neither counted nor offered, and its inputs aren't claimed
/// again. The same holds after a restart.
#[test]
fn a_queued_event_of_an_abandoned_child_does_not_bring_it_back() {
    use dash_spv::EventHandler;
    let (_dir, engine, session, id, paid, parent, child, queued) =
        child_recorded_before_its_event();
    assert!(!queued.is_empty(), "no event was queued");
    abandon(&engine, &session, id, parent.txid());
    let abandoned_coins = held(&engine, &session, id);
    let abandoned = assert_holds(&engine, &session, id, &abandoned_coins, "abandoned");
    for event in &queued {
        session.hub.on_wallet_event(event);
    }
    assert_child_gone(&engine, &session, id, &child, "the event delivered");
    assert_eq!(
        assert_holds(
            &engine,
            &session,
            id,
            &abandoned_coins,
            "the event delivered"
        ),
        abandoned,
        "the event changed the balance"
    );
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    let want = outputs(&paid);
    assert_holds(&engine, &session, id, &want, "the funding scanned");
    assert_child_gone(&engine, &session, id, &child, "the funding scanned");

    let session = reopen(&engine, session);
    assert_child_gone(&engine, &session, id, &child, "reopened");
    assert_holds(&engine, &session, id, &want, "reopened");
}

/// After a restart key-wallet no longer holds the Abandon's dismissal (it
/// keeps it in memory only), so a peer's mempool can bring the payment back
/// and key-wallet counts it again: that event's balance is applied, as on
/// main.
#[test]
fn an_abandoned_payment_seen_again_after_a_restart_applies_its_balance() {
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
    abandon(&engine, &session, id, payment.txid());

    let session = reopen(&engine, session);
    let before = held(&engine, &session, id);
    assert_holds(&engine, &session, id, &before, "reopened");
    mempool(&engine, &session, id, &payment);
    let after = held(&engine, &session, id);
    assert_ne!(before, after, "key-wallet didn't count it again");
    assert_holds(&engine, &session, id, &after, "seen again");
}

/// A height-only row without a height (an unconfirmed coin's) beside a
/// saved payment is no evidence it settled: after the restart the payment
/// is still listed pending and its coin stays out.
#[test]
fn an_unconfirmed_height_only_row_does_not_settle_a_saved_payment() {
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
    let own = super::own_spend_disk_tests::own_scripts(&engine, &session, id);
    let c = super::own_spend_disk_tests::change_of(&payment, &own);
    let txout = payment.output[c.vout as usize].clone();
    let address =
        dashcore::Address::from_script(&txout.script_pubkey, dashcore::Network::Regtest).unwrap();
    let utxo = key_wallet::Utxo::new(c, txout, address, 0, false);
    assert!(!utxo.is_confirmed);
    super::own_spend_disk_tests::store_raw(
        &session,
        id,
        platform_wallet::changeset::CoreChangeSet {
            new_utxos: vec![utxo],
            ..Default::default()
        },
    );

    let session = reopen(&engine, session);
    assert!(
        listed(&engine, &session, id, payment.txid()),
        "settled by the row"
    );
    assert!(!spendable(&engine, &session, id, a), "A is offered");
}

/// An abandoned payment of a coin funded without a record keeps its saved
/// input details after its sent mark goes: two starts after the Abandon it
/// is still listed as a payment, not as a receipt of its change.
#[test]
fn an_abandoned_payment_of_a_coin_funded_without_a_record_stays_a_payment_two_starts_later() {
    let (_dir, engine, session, id, _paid) = wallet_funded_unmined();
    let (session, coin) = coin_funded_without_a_record(&engine, session, id, 10);
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
    abandon(&engine, &session, id, payment.txid());

    let row = |session: &Arc<NetworkSession>| {
        rows(&engine, session, id)
            .into_iter()
            .find(|r| r.0 == payment.txid().to_string())
    };
    let session = reopen(&engine, session);
    let first = row(&session);
    assert!(
        first.as_ref().is_some_and(|r| r.3 < 0),
        "first start: {first:?}"
    );
    let session = reopen(&engine, session);
    let second = row(&session);
    assert!(
        second.as_ref().is_some_and(|r| r.3 < 0),
        "second start: {second:?}"
    );
}

/// A lone payment whose saved copy is damaged, with no other payment
/// pending, keeps its coin out after a restart and a scan of its funding
/// block (the damaged-copy fallback did nothing when no readable payment
/// was pending).
#[test]
fn a_lone_damaged_copy_keeps_its_coin_out() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let damaged = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    let scope = id.to_string();
    (session.live().unwrap().appdb)
        .set_setting(&scope, &format!("senttx:{}", damaged.txid()), Some("zz"))
        .unwrap();

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(!spendable(&engine, &session, id, a), "A is offered");
    assert!(!held(&engine, &session, id).contains(&a), "A is counted");
}

/// The saved copies' query fails while the wallet opens: the sent marks
/// still read, so a payment sent before a crash keeps its coin out. After
/// the restart and a scan of its funding block, A is neither offered nor
/// counted, and its mark stays. Once the copies read again, the payment is
/// listed pending from its copy as usual.
#[test]
fn a_failed_read_of_the_saved_copies_keeps_a_sent_payments_coin_out() {
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
    let path = session.data_dir().join(dw_appdb::APP_DB_FILE);
    let stage = dw_appdb::fault::Stage::Before;
    for _ in 0..2 {
        dw_appdb::fault::arm(&path, "settings.read_senttx:", stage);
    }
    let session = reopen(&engine, session);
    dw_appdb::fault::disarm(&path);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(!spendable(&engine, &session, id, a), "A is offered");
    assert!(!held(&engine, &session, id).contains(&a), "A is counted");
    assert!(
        sent_marks(&session, id).contains(&payment.txid()),
        "the mark went"
    );

    let session = reopen(&engine, session);
    assert!(listed(&engine, &session, id, payment.txid()), "not listed");
    assert!(!spendable(&engine, &session, id, a), "again: A is offered");
}

/// A sent mark whose coin list doesn't read (not text, or a coin that
/// doesn't parse) still holds the payment's coin: it is read from the
/// payment's input details, or else its saved copy. After a restart and a
/// scan of the funding block, A is neither offered nor counted.
#[test]
fn a_sent_mark_whose_coins_do_not_read_keeps_its_coin_out() {
    for (damage, lose_inputs) in [
        ("CAST(X'ff' AS TEXT)", false),
        ("'zz'", false),
        ("CAST(X'ff' AS TEXT)", true),
    ] {
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
        let conn =
            rusqlite::Connection::open(session.data_dir().join(dw_appdb::APP_DB_FILE)).unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let key = format!("sent:{}", payment.txid());
        conn.execute(
            &format!("UPDATE settings_kv SET value = {damage} WHERE scope = ?1 AND key = ?2"),
            [id.to_string(), key],
        )
        .unwrap();
        if lose_inputs {
            conn.execute(
                "DELETE FROM settings_kv WHERE scope = ?1 AND key = ?2",
                [id.to_string(), format!("sentin:{}", payment.txid())],
            )
            .unwrap();
        }
        drop(conn);

        let session = reopen(&engine, session);
        mined(&engine, &session, id, TIP, vec![paid.clone()]);
        let state = format!("{damage}, input details lost: {lose_inputs}");
        assert!(
            !spendable(&engine, &session, id, a),
            "{state}: A is offered"
        );
        assert!(
            !held(&engine, &session, id).contains(&a),
            "{state}: A is counted"
        );
    }
}

/// Remove unconfirmed transactions whose delete of a damaged payment's
/// marks fails says so and doesn't count it: the marks stay, so after a
/// restart its coin is still held, as the payment is still recorded.
#[test]
fn remove_unconfirmed_counts_a_damaged_payment_only_once_its_marks_are_gone() {
    let (_dir, engine, session, id, paid, _damaged) = damaged_payment_whose_marks_did_not_delete();
    let a = OutPoint::new(paid.txid(), 0);
    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(
        !spendable(&engine, &session, id, a),
        "A is offered after a restart"
    );
}

/// Remove unconfirmed transactions run again in the session after its
/// delete failed deletes the damaged payment's marks and counts it, though
/// the payment is already abandoned: after a restart, A is free once its
/// funding is scanned.
#[test]
fn remove_unconfirmed_run_again_in_the_session_deletes_the_marks() {
    let (_dir, engine, session, id, paid, damaged) = damaged_payment_whose_marks_did_not_delete();
    let a = OutPoint::new(paid.txid(), 0);
    let removed = engine.block_on(session.drop_unconfirmed(Some(id)));
    assert_eq!(removed.ok(), Some(1), "run again");
    assert!(
        !sent_marks(&session, id).contains(&damaged.txid()),
        "the marks stay"
    );

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(spendable(&engine, &session, id, a), "A is held");
}

/// The same, when the rerun's sweep fails before the marks' delete: it
/// returns the storage error and the marks stay; running it again deletes
/// them, and after a restart A is free once its funding is scanned.
#[test]
fn remove_unconfirmed_whose_sweep_fails_keeps_the_marks_until_run_again() {
    let (_dir, engine, session, id, paid, damaged) = damaged_payment_whose_marks_did_not_delete();
    let a = OutPoint::new(paid.txid(), 0);
    (session.hub.fail_abandon_sweep).store(true, std::sync::atomic::Ordering::SeqCst);
    let removed = engine.block_on(session.drop_unconfirmed(Some(id)));
    assert!(
        matches!(removed, Err(crate::EngineError::Storage(_))),
        "{removed:?}"
    );
    assert!(
        sent_marks(&session, id).contains(&damaged.txid()),
        "the marks went"
    );
    let removed = engine.block_on(session.drop_unconfirmed(Some(id)));
    assert_eq!(removed.ok(), Some(1), "run again");
    assert!(
        !sent_marks(&session, id).contains(&damaged.txid()),
        "the marks stay"
    );

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(spendable(&engine, &session, id, a), "A is held");
}

/// The same, run again after a restart.
#[test]
fn remove_unconfirmed_run_again_after_a_restart_deletes_the_marks() {
    let (_dir, engine, session, id, paid, damaged) = damaged_payment_whose_marks_did_not_delete();
    let a = OutPoint::new(paid.txid(), 0);
    let session = reopen(&engine, session);
    spv_running(&engine, &session);
    let removed = engine.block_on(session.drop_unconfirmed(Some(id)));
    assert_eq!(removed.ok(), Some(1), "run again");
    assert!(
        !sent_marks(&session, id).contains(&damaged.txid()),
        "the marks stay"
    );

    let session = reopen(&engine, session);
    mined(&engine, &session, id, TIP, vec![paid.clone()]);
    assert!(spendable(&engine, &session, id, a), "A is held");
}

/// A payment of A whose saved copy is damaged, after a restart and a
/// Remove unconfirmed transactions whose delete of its marks failed: the
/// error is returned and the marks stay.
fn damaged_payment_whose_marks_did_not_delete() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
    Transaction,
) {
    let (dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let damaged = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    let scope = id.to_string();
    (session.live().unwrap().appdb)
        .set_setting(&scope, &format!("senttx:{}", damaged.txid()), Some("zz"))
        .unwrap();
    let session = reopen(&engine, session);
    spv_running(&engine, &session);
    let path = session.data_dir().join(dw_appdb::APP_DB_FILE);
    let stage = dw_appdb::fault::Stage::Before;
    dw_appdb::fault::arm(&path, crate::tx_actions::FORGET_SENT_FAULT, stage);
    let removed = engine.block_on(session.drop_unconfirmed(Some(id)));
    dw_appdb::fault::disarm(&path);
    assert!(removed.is_err(), "counted as removed: {removed:?}");
    assert!(
        sent_marks(&session, id).contains(&damaged.txid()),
        "the marks went"
    );
    (dir, engine, session, id, paid, damaged)
}

fn spv_running(engine: &Engine, session: &Arc<NetworkSession>) {
    super::own_spend_disk_tests::start_offline_spv(engine, session);
}
