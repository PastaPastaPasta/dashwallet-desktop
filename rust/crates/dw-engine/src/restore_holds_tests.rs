//! The R3 restore's money gate and holds (Q92 H1–H5) on a real engine:
//! the restored mark is written into the network dir before a reopen, as
//! a restore leaves it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use dashcore::{OutPoint, Transaction, Txid};

use super::own_spend_disk_tests::{
    COIN, TIP, change_of, coins, eventually, own_scripts, prepare, rival, send, spendable,
    spendable_for, wait_stored, wallet_funded_unmined,
};
use super::{balances, instant_lock, mempool, mined, reopen};
use crate::send::{CoinSource, SendFailure};
use crate::session::NetworkSession;
use crate::{Engine, EngineError, TxActionRefusal, WalletId};

fn mark_dir(session: &NetworkSession) -> std::path::PathBuf {
    session
        .data_dir()
        .join(dw_store_upgrade::UPGRADE_BACKUPS_DIR)
}

/// Writes the restored mark, with `pending` as its pending list, and
/// reopens the network on it.
fn restored(
    engine: &Engine,
    session: Arc<NetworkSession>,
    pending: &[(Txid, OutPoint)],
) -> Arc<NetworkSession> {
    let hex = |b: &[u8]| b.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let mut text = String::new();
    for (txid, input) in pending {
        let input = bincode::serde::encode_to_vec(input, bincode::config::standard()).unwrap();
        text += &format!("pending {} {}\n", hex(txid.as_byte_array()), hex(&input));
    }
    restored_with(engine, session, &text)
}

/// [`restored`] with the mark's entry lines `entries`, as written.
fn restored_with(
    engine: &Engine,
    session: Arc<NetworkSession>,
    entries: &str,
) -> Arc<NetworkSession> {
    let dir = mark_dir(&session);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("RESTORED"),
        format!("restored\nformat 1\n{entries}"),
    )
    .unwrap();
    let session = reopen(engine, session);
    assert!(session.restored(), "the gate is on");
    session
}

fn mark_present(session: &NetworkSession) -> bool {
    dw_store_upgrade::restored_mark_present(session.data_dir())
}

fn holds(session: &NetworkSession, id: WalletId) -> BTreeMap<Txid, BTreeSet<OutPoint>> {
    crate::restore_gate::read_holds(&session.live().unwrap().appdb, id).unwrap()
}

fn sent_marks(session: &NetworkSession, id: WalletId) -> BTreeSet<Txid> {
    crate::tx_actions::load_sent(&session.live().unwrap().appdb, id)
        .0
        .into_keys()
        .collect()
}

fn abandoned_marks(session: &NetworkSession, id: WalletId) -> BTreeSet<Txid> {
    (crate::tx_actions::load_abandoned(&session.live().unwrap().appdb, id)
        .0
        .copies)
        .into_iter()
        .map(|(t, _)| t)
        .collect()
}

/// Services read as synced: the watcher runs its drop-dead pass and clears
/// the mark.
fn all_synced(session: &NetworkSession) {
    *session.hub.restore.all_synced.lock().unwrap() = Some(true);
    session.hub.restore.wake.notify_one();
}

fn cleared(session: &NetworkSession) {
    all_synced(session);
    eventually("the restored mark clears", || !session.restored());
    assert!(!mark_present(session), "the mark is gone");
}

/// The wallet's payment `p` of A (one DASH) for half a DASH, with change,
/// unmined, recorded; the network then restored: `p` is held.
fn held_payment() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
    Transaction,
) {
    let (dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    wait_stored(&session, id, &[p.txid()]);
    let session = restored(&engine, session, &[]);
    assert_eq!(
        holds(&session, id).get(&p.txid()),
        Some(&BTreeSet::from([a])),
        "the hold names the payment's input"
    );
    (dir, engine, session, id, paid, p)
}

fn utxo_row(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    coin: OutPoint,
) -> Option<crate::CoinInfo> {
    let rows = engine
        .block_on(session.utxos(id, crate::CoinFilter::default()))
        .unwrap();
    rows.into_iter().find(|r| r.outpoint == coin)
}

/// `p`'s change is neither offered nor counted spendable, and shows as
/// pending.
fn assert_change_held(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    p: &Transaction,
    state: &str,
) {
    let change = change_of(p, &own_scripts(engine, session, id));
    let value = p.output[change.vout as usize].value;
    assert!(
        !spendable_for(engine, session, id, change, COIN / 4),
        "{state}: the change is offered"
    );
    let row = utxo_row(engine, session, id, change).expect("the change is listed");
    assert!(row.pending_unproven && !row.spendable, "{state}: {row:?}");
    eventually("the change counts pending", || {
        let b = balances(session, id);
        b.confirmed == 11 * COIN && b.unconfirmed >= value
    });
    let b = balances(session, id);
    assert_eq!(b.total, 11 * COIN + value, "{state}: {b:?}");
}

/// H1: a restored pending payment's change is never offered and counts
/// pending, not spendable, once the gate is off and after a restart; the
/// wallet's other coins are offered.
#[test]
fn a_held_payments_change_is_not_offered_and_counts_pending_across_a_restart() {
    let (_dir, engine, session, id, paid, p) = held_payment();
    cleared(&session);
    assert!(
        !holds(&session, id).is_empty(),
        "the hold outlives the mark"
    );
    assert_change_held(&engine, &session, id, &p, "the gate off");
    let b = OutPoint::new(paid.txid(), 1);
    assert!(
        spendable(&engine, &session, id, b),
        "another coin is offered"
    );
    assert!(
        !utxo_row(&engine, &session, id, b).unwrap().pending_unproven,
        "another coin is not flagged"
    );

    let session = reopen(&engine, session);
    assert!(!session.restored());
    assert_change_held(&engine, &session, id, &p, "after a restart");
}

/// H1 for an InstantSend-locked payment, whose change key-wallet counts
/// confirmed: the hold moves it to pending.
#[test]
fn a_held_locked_payments_change_counts_pending_across_a_restart() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    instant_lock(&engine, &session, id, &p);
    wait_stored(&session, id, &[p.txid()]);
    let session = restored(&engine, session, &[]);
    assert!(holds(&session, id).contains_key(&p.txid()));
    cleared(&session);
    assert_change_held(&engine, &session, id, &p, "the gate off");
    let session = reopen(&engine, session);
    assert_change_held(&engine, &session, id, &p, "after a restart");
}

/// Money gate: while the mark is set, a send, Abandon, Resend and Remove
/// Unconfirmed are refused by name.
#[test]
fn every_funds_action_is_refused_by_name_while_restored() {
    let (_dir, engine, session, id, paid, p) = held_payment();
    let b = OutPoint::new(paid.txid(), 1);
    let (_draft, prepared) = prepare(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![b]),
        COIN / 2,
        false,
        &Arc::new(super::own_spend_disk_tests::Accept::default()),
    );
    assert!(
        matches!(prepared, Err(EngineError::Send(SendFailure::Restored))),
        "{prepared:?}"
    );
    let refused = |e: Result<(), EngineError>| {
        matches!(
            e,
            Err(EngineError::TxActionRefused(TxActionRefusal::Restored))
        )
    };
    let txid = p.txid();
    assert!(refused(
        engine.block_on(session.abandon_transaction(id, txid))
    ));
    assert!(refused(
        engine.block_on(session.resend_transaction(id, txid))
    ));
    assert!(refused(
        engine
            .block_on(session.drop_unconfirmed(Some(id)))
            .map(|_| ())
    ));
    assert!(
        matches!(
            engine.block_on(session.start_mixing(id)),
            Err(EngineError::CoinJoin(crate::CoinJoinFailure::Restored))
        ),
        "mixing"
    );
    let restored = |e: &EngineError| matches!(e, EngineError::Restored);
    let guard = engine.block_on(session.replay_guard(id, 0));
    assert!(guard.as_ref().is_err_and(restored), "{guard:?}");
    let resend = engine.block_on(session.resend_mixed_coins(id, None));
    assert!(resend.as_ref().is_err_and(restored), "{resend:?}");
    let mut unsigned = p.clone();
    unsigned
        .input
        .iter_mut()
        .for_each(|i| i.script_sig = Default::default());
    let psbt = dw_psbt::PartiallySignedTransaction::from_unsigned_tx(unsigned).unwrap();
    let broadcast = engine.block_on(session.broadcast_psbt(psbt));
    assert!(broadcast.as_ref().is_err_and(restored), "{broadcast:?}");
    let lease = engine.block_on(session.begin_lease(id, crate::platform::FlowKind::Shield, &[]));
    assert!(
        matches!(lease, Err(crate::platform::LeaseError::Restored)),
        "{:?}",
        lease.err()
    );
    let vote = engine.block_on(session.begin_lease(id, crate::platform::FlowKind::Voting, &[]));
    assert!(!matches!(vote, Err(crate::platform::LeaseError::Restored)));
    assert!(holds(&session, id).contains_key(&txid));
    assert!(abandoned_marks(&session, id).is_empty());
}

/// The wallet's payment `p` of A handed to the network and never recorded
/// (the app quit first), then the network restored: `p` is held by its
/// sent mark alone.
fn held_unrecorded_payment() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
    Transaction,
) {
    let (dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    let session = restored(&engine, session, &[]);
    assert_eq!(
        holds(&session, id).get(&p.txid()),
        Some(&BTreeSet::from([a]))
    );
    (dir, engine, session, id, paid, p)
}

/// H2 and the money gate: once the mark clears, Abandon is allowed with
/// the hold still there. A failed marks write leaves the hold and no
/// abandoned mark (A keeps its claim); a written one leaves the abandoned
/// mark and no hold (never both, never neither).
#[test]
fn abandon_after_the_clear_swaps_the_hold_for_its_mark_in_one_write() {
    let (_dir, engine, session, id, paid, p) = held_unrecorded_payment();
    let a = OutPoint::new(paid.txid(), 0);
    let txid = p.txid();
    let extras = engine.block_on(session.tx_detail_extras(id, txid)).unwrap();
    assert!(extras.restore_held && extras.can_abandon, "{extras:?}");
    cleared(&session);
    super::own_spend_disk_tests::start_offline_spv(&engine, &session);

    session
        .hub
        .fail_abandon_mark
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let failed = engine.block_on(session.abandon_transaction(id, txid));
    assert!(failed.is_err(), "{failed:?}");
    assert!(
        !session
            .hub
            .fail_abandon_mark
            .load(std::sync::atomic::Ordering::SeqCst),
        "the fault was hit: {failed:?}"
    );
    assert!(holds(&session, id).contains_key(&txid), "the hold stays");
    assert!(abandoned_marks(&session, id).is_empty(), "no mark");
    assert!(sent_marks(&session, id).contains(&txid), "the claim stays");
    assert!(!spendable(&engine, &session, id, a), "A is offered");

    engine
        .block_on(session.abandon_transaction(id, txid))
        .unwrap();
    assert!(!holds(&session, id).contains_key(&txid), "the hold goes");
    assert!(abandoned_marks(&session, id).contains(&txid), "marked");
    let extras = engine.block_on(session.tx_detail_extras(id, txid)).unwrap();
    assert!(!extras.restore_held, "{extras:?}");
    let session = reopen(&engine, session);
    assert!(holds(&session, id).is_empty());
    assert!(abandoned_marks(&session, id).contains(&txid));
}

/// H2: a held payment mined deletes its hold with the block.
#[test]
fn a_held_payment_mined_deletes_its_hold() {
    let (_dir, engine, session, id, paid, p) = held_payment();
    mined(&engine, &session, id, TIP, vec![paid, p.clone()]);
    eventually("the hold goes", || {
        !holds(&session, id).contains_key(&p.txid())
    });
    cleared(&session);
    let change = change_of(&p, &own_scripts(&engine, &session, id));
    assert!(
        spendable_for(&engine, &session, id, change, COIN / 4),
        "the change is offered"
    );
}

/// A held payment a peer relays again after the restore, so key-wallet
/// records it in this session (it rebuilds no record at a load).
fn held_payment_relayed() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
    Transaction,
) {
    let (dir, engine, session, id, paid, p) = held_unrecorded_payment();
    mempool(&engine, &session, id, &p);
    wait_stored(&session, id, &[p.txid()]);
    assert!(holds(&session, id).contains_key(&p.txid()));
    (dir, engine, session, id, paid, p)
}

/// H2: a recorded held payment beaten by a mined spend of its coin during
/// the sync after the restore loses its hold with its marks, and its
/// outputs are never offered.
#[test]
fn a_held_payment_a_mined_spend_beat_loses_its_hold_and_its_outputs() {
    let (_dir, engine, session, id, paid, p) = held_payment_relayed();
    let winner = rival(&engine, &session, id, &paid, 0);
    mined(&engine, &session, id, TIP, vec![paid]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    eventually("the hold goes with the sweep", || {
        !holds(&session, id).contains_key(&p.txid())
    });
    assert!(
        !sent_marks(&session, id).contains(&p.txid()),
        "the marks go"
    );
    let held: BTreeSet<OutPoint> = coins(&engine, &session, id)
        .into_iter()
        .map(|c| c.0)
        .collect();
    assert!(
        !held.iter().any(|o| o.txid == p.txid()),
        "the payment's outputs are held: {held:?}"
    );
    cleared(&session);
    let change = change_of(&p, &own_scripts(&engine, &session, id));
    assert!(
        utxo_row(&engine, &session, id, change).is_none(),
        "the change is listed"
    );
    assert!(
        !spendable_for(&engine, &session, id, change, COIN / 4),
        "the change is offered"
    );
}

/// H2 batch invariant on the sweep: a failed write of the sweep's marks
/// leaves the hold with the sent mark (one write, all or nothing), and the
/// dropped payment's outputs are never offered.
#[test]
fn a_failed_sweep_write_keeps_the_hold_with_the_sent_mark() {
    let (_dir, engine, session, id, paid, p) = held_payment_relayed();
    let winner = rival(&engine, &session, id, &paid, 0);
    let path = session.data_dir().join(dw_appdb::APP_DB_FILE);
    dw_appdb::fault::arm(
        &path,
        crate::tx_actions::FORGET_SENT_FAULT,
        dw_appdb::fault::Stage::Before,
    );
    mined(&engine, &session, id, TIP, vec![paid]);
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    eventually("the payment is dropped", || {
        !coins(&engine, &session, id)
            .iter()
            .any(|c| c.0.txid == p.txid())
    });
    dw_appdb::fault::disarm(&path);
    assert!(
        holds(&session, id).contains_key(&p.txid()),
        "the hold stays"
    );
    assert!(
        sent_marks(&session, id).contains(&p.txid()),
        "with the mark"
    );
    let change = change_of(&p, &own_scripts(&engine, &session, id));
    assert!(utxo_row(&engine, &session, id, change).is_none());
}

/// H3: a payment known only by its sent mark (the app quit before its
/// record was stored) is held, its coin with it.
#[test]
fn a_payment_known_only_by_its_sent_mark_is_held() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    let session = restored(&engine, session, &[]);
    assert_eq!(
        holds(&session, id).get(&p.txid()),
        Some(&BTreeSet::from([a]))
    );
    cleared(&session);
    assert!(!spendable(&engine, &session, id, a), "A is offered");
}

/// H3 cross-check: a payment the restored mark lists with a coin the
/// history doesn't name for it is held with that coin too.
#[test]
fn the_marks_pending_list_adds_its_inputs_to_the_hold() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let extra = OutPoint::new(paid.txid(), 5);
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    wait_stored(&session, id, &[p.txid()]);
    let session = restored(&engine, session, &[(p.txid(), extra)]);
    assert_eq!(
        holds(&session, id).get(&p.txid()),
        Some(&BTreeSet::from([a, extra]))
    );
    cleared(&session);
    assert!(
        !spendable(&engine, &session, id, extra),
        "the listed coin is offered"
    );
}

/// H4: all-Synced alone doesn't clear the mark; it clears once the
/// drop-dead pass that started after it completes, and the gate opens.
#[test]
fn the_mark_clears_only_after_the_drop_dead_pass() {
    let (_dir, engine, session, id, paid, _p) = held_payment();
    let dead = Txid::from_byte_array([0x24; 32]);
    (session.live().unwrap().appdb)
        .set_setting(&id.to_string(), &format!("dropped:{dead}"), Some(""))
        .unwrap();
    let session = reopen(&engine, session);
    assert!(session.restored());
    let pause = Arc::new(crate::coins::TestPause::default());
    *session.hub.restore.pause_pass.lock().unwrap() = Some(Arc::clone(&pause));
    all_synced(&session);
    assert!(
        pause.wait_reached(std::time::Duration::from_secs(20)),
        "the pass started"
    );
    assert!(session.restored(), "cleared during the pass");
    assert!(mark_present(&session), "the mark went during the pass");
    assert!(session.hub.restore.pass_runs.lock().unwrap().is_empty());
    pause.release.notify_one();
    eventually("the mark clears", || !session.restored());
    assert!(!mark_present(&session));
    assert_eq!(
        *session.hub.restore.pass_runs.lock().unwrap(),
        vec![(id, BTreeSet::from([dead]))],
        "the pass ran with the load's seeds before the clear"
    );
    let b = OutPoint::new(paid.txid(), 1);
    assert!(spendable(&engine, &session, id, b), "sends are allowed");
}

/// H4: the mark stays while the services aren't synced.
#[test]
fn the_mark_stays_until_all_synced() {
    let (_dir, _engine, session, _id, _paid, _p) = held_payment();
    session.hub.restore.wake.notify_one();
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert!(
        session.restored() && mark_present(&session),
        "offline, Core is not caught up"
    );
    *session.hub.restore.all_synced.lock().unwrap() = Some(false);
    session.hub.restore.wake.notify_one();
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert!(session.restored() && mark_present(&session));
}

/// H5: a held InstantSend-locked payment mined loses its hold; a
/// truncation that takes its block away (it stays, locked, out of a block)
/// while the mark is set writes the hold again.
#[test]
fn a_truncation_that_unmines_a_held_payment_holds_it_again() {
    let (_dir, engine, session, id, paid, p) = held_payment();
    instant_lock(&engine, &session, id, &p);
    mined(&engine, &session, id, TIP + 1, vec![paid, p.clone()]);
    eventually("the hold goes", || {
        !holds(&session, id).contains_key(&p.txid())
    });
    super::deliver(&engine, &session, id, move |wm| {
        Box::pin(async move {
            use platform_wallet::key_wallet_manager::WalletInterface;
            wm.truncate_above(TIP).unwrap();
        })
    });
    eventually("the hold is written again", || {
        holds(&session, id).contains_key(&p.txid())
    });
}

/// Q102 102-2 (H3 before H4): while the holds a complete load builds are
/// not written, all-Synced and the pass leave the mark set; a later load
/// that writes them lets it clear.
#[test]
fn the_mark_stays_until_a_load_has_written_the_holds() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    wait_stored(&session, id, &[p.txid()]);
    let path = session.data_dir().join(dw_appdb::APP_DB_FILE);
    dw_appdb::fault::arm(
        &path,
        crate::restore_gate::BUILD_FAULT,
        dw_appdb::fault::Stage::Before,
    );
    let session = restored(&engine, session, &[]);
    dw_appdb::fault::disarm(&path);
    assert!(holds(&session, id).is_empty(), "the holds were written");
    session.hub.rebuild_restore_holds_after_truncation(id);
    assert!(
        holds(&session, id).contains_key(&p.txid()),
        "H5 writes rows"
    );
    all_synced(&session);
    std::thread::sleep(std::time::Duration::from_secs(5));
    assert!(
        session.restored() && mark_present(&session),
        "cleared unheld"
    );

    let session = reopen(&engine, session);
    assert!(holds(&session, id).contains_key(&p.txid()));
    cleared(&session);
}

/// Q102 102-3: a mark entry that doesn't read opens the wallet with the
/// gate closed and the history's unmined own payments held.
#[test]
fn a_corrupt_mark_entry_opens_the_wallet_with_the_gate_closed() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    wait_stored(&session, id, &[p.txid()]);
    let session = restored_with(&engine, session, "pending zz zz\n");
    assert!(dw_store_upgrade::restored_mark(session.data_dir()).is_err());
    assert!(balances(&session, id).total > 0, "the wallet is open");
    assert_eq!(
        holds(&session, id).get(&p.txid()),
        Some(&BTreeSet::from([a]))
    );
    let b = OutPoint::new(paid.txid(), 1);
    let (_draft, prepared) = prepare(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![b]),
        COIN / 2,
        false,
        &Arc::new(super::own_spend_disk_tests::Accept::default()),
    );
    assert!(
        matches!(prepared, Err(EngineError::Send(SendFailure::Restored))),
        "{prepared:?}"
    );
    let change = change_of(&p, &own_scripts(&engine, &session, id));
    let row = utxo_row(&engine, &session, id, change).expect("listed");
    assert!(row.pending_unproven && !row.spendable, "{row:?}");
    cleared(&session);
    assert_change_held(&engine, &session, id, &p, "the gate off");
}

/// Q102 102-4: an entry of the mark whose payment the live history lacks
/// (a foreign set kept live) holds nothing, and its coin is offered once
/// the mark clears.
#[test]
fn a_mark_entry_the_history_lacks_holds_nothing() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let foreign_payment = Txid::from_byte_array([0x42; 32]);
    let c = OutPoint::new(paid.txid(), 3);
    let session = restored(&engine, session, &[(foreign_payment, c)]);
    let mark = dw_store_upgrade::restored_mark(session.data_dir())
        .unwrap()
        .unwrap();
    assert_eq!(mark.pending.len(), 1);
    assert!(holds(&session, id).is_empty(), "{:?}", holds(&session, id));
    let row = utxo_row(&engine, &session, id, c).expect("listed");
    assert!(!row.pending_unproven, "{row:?}");
    cleared(&session);
    assert!(spendable(&engine, &session, id, c), "the coin is offered");
    let session = reopen(&engine, session);
    assert!(spendable(&engine, &session, id, c), "after a restart");
}

/// A wallet registered but not loaded while the mark was set: the clear
/// leaves it a build mark, and its first complete load holds its pending
/// payments as if the mark were still set, ending the mark in that write.
#[test]
fn a_wallet_loaded_after_the_clear_is_held_at_its_load() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    wait_stored(&session, id, &[p.txid()]);
    let _stays_on_the_list = engine.block_on(session.create_wallet(12)).unwrap();
    engine
        .block_on(session.set_load_on_startup(id, false))
        .unwrap();
    let session = restored(&engine, session, &[]);
    assert!(
        session.live().unwrap().store.is_unloaded(&id.0),
        "loaded at the start"
    );
    assert!(holds(&session, id).is_empty());
    cleared(&session);
    let appdb = Arc::clone(&session.live().unwrap().appdb);
    assert!(crate::restore_gate::build_pending(&appdb, id).unwrap());

    engine.block_on(session.load_wallet(id)).unwrap();
    assert_eq!(
        holds(&session, id).get(&p.txid()),
        Some(&BTreeSet::from([a]))
    );
    assert!(!crate::restore_gate::build_pending(&appdb, id).unwrap());
    assert_change_held(&engine, &session, id, &p, "loaded after the clear");
}

/// A wallet loaded after the clear whose build fails holds every unmined
/// own payment until a load writes its holds.
#[test]
fn a_late_wallet_whose_build_fails_holds_its_pending_payments() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let p = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &p);
    wait_stored(&session, id, &[p.txid()]);
    let _stays_on_the_list = engine.block_on(session.create_wallet(12)).unwrap();
    engine
        .block_on(session.set_load_on_startup(id, false))
        .unwrap();
    let session = restored(&engine, session, &[]);
    cleared(&session);
    let appdb = Arc::clone(&session.live().unwrap().appdb);
    let path = session.data_dir().join(dw_appdb::APP_DB_FILE);
    dw_appdb::fault::arm(
        &path,
        crate::restore_gate::BUILD_FAULT,
        dw_appdb::fault::Stage::Before,
    );
    engine.block_on(session.load_wallet(id)).unwrap();
    dw_appdb::fault::disarm(&path);
    assert!(holds(&session, id).is_empty(), "the holds were written");
    assert!(crate::restore_gate::build_pending(&appdb, id).unwrap());
    assert_change_held(&engine, &session, id, &p, "the build failed");

    engine.block_on(session.unload_wallet(id)).unwrap();
    engine.block_on(session.load_wallet(id)).unwrap();
    assert!(holds(&session, id).contains_key(&p.txid()));
    assert!(!crate::restore_gate::build_pending(&appdb, id).unwrap());
    assert_change_held(&engine, &session, id, &p, "built at the next load");
}

/// H4: a clear that errs (the crate's answer to a mark it can't read)
/// leaves the mark set and the gate closed; the next all-Synced and pass
/// clear it.
#[test]
fn a_failed_clear_keeps_the_gate_closed_and_retries_after_a_pass() {
    let (_dir, engine, session, id, _paid, _p) = held_payment();
    let dead = Txid::from_byte_array([0x25; 32]);
    (session.live().unwrap().appdb)
        .set_setting(&id.to_string(), &format!("dropped:{dead}"), Some(""))
        .unwrap();
    let session = reopen(&engine, session);
    let restore = &session.hub.restore;
    restore
        .clear_fails
        .store(1, std::sync::atomic::Ordering::SeqCst);
    let first = Arc::new(crate::coins::TestPause::default());
    *restore.pause_pass.lock().unwrap() = Some(Arc::clone(&first));
    all_synced(&session);
    assert!(first.wait_reached(std::time::Duration::from_secs(20)));
    let second = Arc::new(crate::coins::TestPause::default());
    *restore.pause_pass.lock().unwrap() = Some(Arc::clone(&second));
    first.release.notify_one();
    assert!(
        second.wait_reached(std::time::Duration::from_secs(20)),
        "no retry after the failed clear"
    );
    assert_eq!(
        restore
            .clear_fails
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(
        session.restored() && mark_present(&session),
        "the failed clear opened the gate"
    );
    assert!(matches!(
        session.refuse_while_restored(),
        Err(EngineError::Restored)
    ));
    second.release.notify_one();
    eventually("the retry clears", || !session.restored());
    assert!(!mark_present(&session));
    assert_eq!(
        restore.pass_runs.lock().unwrap().len(),
        2,
        "the retry ran its own pass"
    );
}

/// The gate reads on before the network's dir is set.
#[test]
fn a_gate_without_its_dir_reads_restored() {
    assert!(crate::restore_gate::RestoreGate::default().is_restored());
}

/// M1: a network opened restored has the gate on before the masternode
/// withdrawals' recheck starts, and the recheck does not run.
#[test]
fn the_withdrawal_recheck_starts_behind_the_gate() {
    let (_dir, _engine, session, _id, _paid, _p) = held_payment();
    let restore = &session.hub.restore;
    assert_eq!(
        *restore.restored_at_recheck_start.lock().unwrap(),
        Some(true),
        "the recheck started with the gate open"
    );
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert_eq!(
        restore
            .recheck_runs
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a recheck ran while restored"
    );
}
