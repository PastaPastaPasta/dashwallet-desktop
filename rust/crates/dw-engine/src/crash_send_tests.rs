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

    engine.block_on(session.start_spv()).unwrap();
    super::own_spend_disk_tests::eventually("SPV runs", || {
        session.spv_state().unwrap() == crate::SpvState::Running
    });
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
    assert!(
        engine
            .block_on(draft.broadcast(Arc::clone(&prepared)))
            .is_err()
    );
    dw_appdb::fault::disarm(&path);
    let txid: Txid = prepared.summary().txid.parse().unwrap();
    assert!(sent_marks(&session, id).contains(&txid), "the mark stays");
    let abandoned = engine.block_on(draft.abandon(prepared));
    assert!(
        matches!(
            abandoned,
            Err(crate::EngineError::Send(
                crate::send::SendFailure::PreparedTxSpent
            ))
        ),
        "{abandoned:?}"
    );
    assert!(!spendable(&engine, &session, id, a), "its coin is offered");
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

    crate::tx_actions::stop_after_sweep::arm(&*session.live().unwrap().persister);
    let (s, txid) = (Arc::clone(&session), payment.txid());
    let stopped = engine.block_on(session.on_runtime(async move {
        let manager = s.manager()?;
        s.abandon_inner(&manager, id, txid).await
    }));
    assert!(stopped.is_err(), "stopped after the sweep: {stopped:?}");

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
