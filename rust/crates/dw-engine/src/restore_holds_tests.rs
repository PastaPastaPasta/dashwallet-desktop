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
    // Pending as unconfirmed, or as withheld when the line's withhold
    // (`crate::withhold`) also keeps it out of every bucket.
    eventually("the change counts pending", || {
        let b = balances(session, id);
        b.confirmed == 11 * COIN && b.unconfirmed + b.withheld >= value
    });
    let b = balances(session, id);
    assert_eq!(b.total + b.withheld, 11 * COIN + value, "{state}: {b:?}");
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
    let resolve = engine.block_on(session.resolve_stuck_funding(
        id,
        "registration/1/funding".into(),
        String::new(),
    ));
    assert!(
        resolve.as_ref().is_err_and(restored),
        "{:?}",
        resolve.as_ref().err()
    );
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
/// mark, and the hold's row stays and holds nothing, across a restart.
#[test]
fn abandon_after_the_clear_marks_the_payment_and_its_hold_holds_nothing() {
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
    assert!(holds(&session, id).contains_key(&txid), "the row stays");
    assert!(abandoned_marks(&session, id).contains(&txid), "marked");
    let extras = engine.block_on(session.tx_detail_extras(id, txid)).unwrap();
    assert!(!extras.restore_held, "{extras:?}");
    assert!(!in_effect(&session, id, txid), "the row holds nothing");
    let session = reopen(&engine, session);
    assert!(holds(&session, id).contains_key(&txid));
    assert!(abandoned_marks(&session, id).contains(&txid));
    let extras = engine.block_on(session.tx_detail_extras(id, txid)).unwrap();
    assert!(!extras.restore_held, "after a restart: {extras:?}");
    assert!(!in_effect(&session, id, txid), "after a restart");
}

fn in_effect(session: &NetworkSession, id: WalletId, txid: Txid) -> bool {
    (session.hub.history).read_wallet(&id, |h| h.restore_holds_in_effect().contains_key(&txid))
}

/// An abandon key-wallet refuses (it holds the root as settled) leaves the
/// hold, its row and in memory: the payment's change stays held.
#[test]
fn a_refused_abandon_keeps_the_hold() {
    use std::sync::atomic::Ordering::SeqCst;
    let (_dir, engine, session, id, _paid, p) = held_payment_relayed();
    let txid = p.txid();
    cleared(&session);
    super::own_spend_disk_tests::start_offline_spv(&engine, &session);
    session.hub.refuse_next_abandon.store(true, SeqCst);

    let refused = engine.block_on(session.abandon_transaction(id, txid));
    assert!(
        matches!(
            refused,
            Err(EngineError::TxActionRefused(TxActionRefusal::Confirmed))
        ),
        "{refused:?}"
    );
    assert_eq!(
        session.hub.refused_abandon_undos.load(SeqCst),
        1,
        "the refused abandon's undo ran"
    );
    assert!(holds(&session, id).contains_key(&txid), "the row stays");
    let extras = engine.block_on(session.tx_detail_extras(id, txid)).unwrap();
    assert!(extras.restore_held, "the hold stays in memory: {extras:?}");
    let change = change_of(&p, &own_scripts(&engine, &session, id));
    assert!(
        !spendable_for(&engine, &session, id, change, COIN / 4),
        "the change is offered"
    );
}

/// A refused abandon whose undo write then fails loses no hold: the row
/// was never deleted, so once the payment's InstantSend lock (what
/// key-wallet's refusal stands for) is heard, a restart after the clear
/// still holds its change.
#[test]
fn a_refused_abandon_with_a_failed_undo_keeps_the_hold_across_a_restart() {
    use std::sync::atomic::Ordering::SeqCst;
    let (_dir, engine, session, id, _paid, p) = held_payment_relayed();
    let txid = p.txid();
    cleared(&session);
    super::own_spend_disk_tests::start_offline_spv(&engine, &session);
    session.hub.refuse_next_abandon.store(true, SeqCst);
    session.hub.fail_abandon_undo.store(true, SeqCst);

    let refused = engine.block_on(session.abandon_transaction(id, txid));
    assert!(
        matches!(
            refused,
            Err(EngineError::TxActionRefused(TxActionRefusal::Confirmed))
        ),
        "{refused:?}"
    );
    assert_eq!(
        session.hub.refused_abandon_undos.load(SeqCst),
        1,
        "the refused abandon's undo ran"
    );
    assert!(
        !session.hub.fail_abandon_undo.load(SeqCst),
        "the undo write failed"
    );
    assert!(holds(&session, id).contains_key(&txid), "the row stays");
    instant_lock(&engine, &session, id, &p);
    let session = reopen(&engine, session);
    assert!(!session.restored(), "the mark stays cleared");
    assert!(
        holds(&session, id).contains_key(&txid),
        "the row is on disk after a restart"
    );
    let extras = engine.block_on(session.tx_detail_extras(id, txid)).unwrap();
    assert!(
        extras.restore_held,
        "the hold is in memory after a restart: {extras:?}"
    );
    assert_change_held(&engine, &session, id, &p, "after a restart");
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

#[test]
fn the_final_pass_drops_a_late_descendant_of_a_store_dropped_claim() {
    use super::store_repair_tests::{claim_on_a_held_coin, dropped, stored};

    let (_dir, engine, session, id) = super::engine_wallet();
    let (session, _, claim) = claim_on_a_held_coin(&engine, session, id, &[]);
    let (vout, output) = (1, &claim.output[1]);
    let owner =
        dashcore::Address::from_script(&output.script_pubkey, dashcore::Network::Regtest).unwrap();
    let receive = super::own_spend_disk_tests::fresh_address(&engine, &session, id);
    let child = super::signed_by_wallet(
        &engine,
        &session,
        id,
        &owner,
        super::spend_of(
            OutPoint::new(claim.txid(), vout),
            vec![(receive.script_pubkey(), output.value - 1_000)],
        ),
    );
    let session = restored_with(&engine, session, "");
    assert!(!stored(session.data_dir(), id, claim.txid()));
    (session.hub.skip_dead_hook).store(true, std::sync::atomic::Ordering::SeqCst);
    mempool(&engine, &session, id, &child);
    eventually("the late child is pending before the pass", || {
        !dropped(&engine, &session, id, child.txid())
    });
    cleared(&session);
    assert!(dropped(&engine, &session, id, child.txid()));
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

/// A payment made after the snapshot can still be mined even when no peer
/// relays it after restoring. Its input stays held across clear and restart.
#[test]
fn a_payment_missing_from_restored_history_keeps_its_input_held() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let foreign_payment = Txid::from_byte_array([0x42; 32]);
    let c = OutPoint::new(paid.txid(), 3);
    let session = restored(&engine, session, &[(foreign_payment, c)]);
    let mark = dw_store_upgrade::restored_mark(session.data_dir())
        .unwrap()
        .unwrap();
    assert_eq!(mark.pending.len(), 1);
    assert_eq!(
        holds(&session, id).get(&foreign_payment),
        Some(&BTreeSet::from([c]))
    );
    let row = utxo_row(&engine, &session, id, c).expect("listed");
    assert!(row.pending_unproven, "{row:?}");
    cleared(&session);
    assert!(!spendable(&engine, &session, id, c), "the coin is offered");
    let session = reopen(&engine, session);
    assert!(!spendable(&engine, &session, id, c), "after a restart");
}

#[test]
fn a_confirmed_conflict_suppresses_a_missing_payments_hold_until_reorg() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let payment = Txid::from_byte_array([0x42; 32]);
    let input = OutPoint::new(paid.txid(), 0);
    let other = OutPoint::new(paid.txid(), 3);
    let session = restored(&engine, session, &[(payment, input), (payment, other)]);
    let winner = rival(&engine, &session, id, &paid, 0);
    mined(&engine, &session, id, TIP, vec![paid]);
    cleared(&session);
    assert!(!spendable(&engine, &session, id, other));
    mined(&engine, &session, id, TIP + 1, vec![winner]);
    eventually("the confirmed conflict releases the other input", || {
        spendable(&engine, &session, id, other)
    });
    super::deliver(&engine, &session, id, move |wm| {
        Box::pin(async move {
            use platform_wallet::key_wallet_manager::WalletInterface;
            wm.truncate_above(TIP).unwrap();
        })
    });
    eventually("the reorg reactivates the missing payment's hold", || {
        !spendable(&engine, &session, id, other)
    });
}

#[test]
fn an_unloaded_wallet_keeps_marked_inputs_after_clear_and_restart() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let payment = Txid::from_byte_array([0x42; 32]);
    let input = OutPoint::new(paid.txid(), 3);
    let _other = engine.block_on(session.create_wallet(12)).unwrap();
    engine
        .block_on(session.set_load_on_startup(id, false))
        .unwrap();
    let session = restored(&engine, session, &[(payment, input)]);
    assert!(session.live().unwrap().store.is_unloaded(&id.0));
    cleared(&session);
    let session = reopen(&engine, session);
    engine.block_on(session.load_wallet(id)).unwrap();
    assert_eq!(
        holds(&session, id).get(&payment),
        Some(&BTreeSet::from([input]))
    );
    assert!(!spendable(&engine, &session, id, input));
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

/// Closes the network and opens it again, Platform off when `no_platform`.
fn reopen_with(
    engine: &Engine,
    session: Arc<NetworkSession>,
    no_platform: bool,
) -> Arc<NetworkSession> {
    *session.close_load_wait.lock().unwrap() = Some(super::LOAD_WAIT);
    drop(session);
    assert!(
        engine
            .block_on(engine.close_network(crate::DashNetwork::Regtest))
            .unwrap()
    );
    let options = crate::SessionOptions {
        no_platform,
        ..super::offline()
    };
    let session = engine
        .block_on(engine.open_network(crate::DashNetwork::Regtest, options))
        .unwrap();
    (session.platform.funds_off_in_tests).store(true, std::sync::atomic::Ordering::SeqCst);
    session
}

fn waiting(engine: &Engine, session: &NetworkSession) -> Option<Vec<crate::RestoreWait>> {
    let status = engine.block_on(session.restore_status()).unwrap();
    status.map(|s| s.waiting)
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

/// The asset-lock catch-up's resume does not run while restored, and does
/// once the mark clears.
#[test]
fn an_asset_lock_resume_waits_for_the_clear() {
    use crate::platform::lease::resume::{Resumed, Resumer};
    let (_dir, engine, session, id, _paid, _p) = held_payment();
    let tracked = crate::platform::lease::mode_b::SessionTracked {
        session: Arc::downgrade(&session),
    };
    let txid = Txid::from_byte_array([0x31; 32]);
    assert_eq!(engine.block_on(tracked.resume(id, txid)), Resumed::NotRun);
    cleared(&session);
    assert_eq!(engine.block_on(tracked.resume(id, txid)), Resumed::Ran);
}

/// A proposal fee's resend is refused while restored.
#[test]
fn a_proposal_fee_resend_is_refused_while_restored() {
    let (_dir, engine, session, id, _paid, _p) = held_payment();
    let refused = engine.block_on(session.governance().resend_fee(id, &[0x42; 32]));
    assert!(
        matches!(
            refused,
            Err(crate::governance::GovernanceError::Engine(
                EngineError::Restored
            ))
        ),
        "{refused:?}"
    );
}

/// The delay after failed clears doubles from the watcher's look to a
/// minute.
#[test]
fn a_failed_clears_delay_grows_to_a_minute() {
    let secs: Vec<u64> = (1..=7)
        .map(|n| crate::restore_gate::clear_backoff(n).as_secs())
        .collect();
    assert_eq!(secs, [4, 8, 16, 32, 60, 60, 60]);
}

/// S2: after a failed clear, wakes do not try it again before its delay;
/// a retry does, at once, and the status names the failure.
#[test]
fn a_failed_clear_waits_out_its_delay_unless_retried() {
    use std::sync::atomic::Ordering::SeqCst;
    let (_dir, engine, session, _id, _paid, _p) = held_payment();
    let session = reopen_with(&engine, session, true);
    let restore = &session.hub.restore;
    restore.clear_fails.store(u32::MAX, SeqCst);
    *restore.backoff.lock().unwrap() = Some(std::time::Duration::from_secs(600));
    all_synced(&session);
    eventually("the first clear fails", || {
        restore.clear_attempts.load(SeqCst) == 1
            && engine
                .block_on(session.restore_status())
                .unwrap()
                .is_some_and(|s| s.clear_failed)
    });
    for _ in 0..10 {
        restore.wake.notify_one();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(
        restore.clear_attempts.load(SeqCst),
        1,
        "a wake tried the clear again within its delay"
    );
    assert!(session.restored() && mark_present(&session));
    engine.block_on(session.restore_retry()).unwrap();
    eventually("the retry tries the clear", || {
        restore.clear_attempts.load(SeqCst) == 2
    });
    assert!(session.restored(), "a failed clear opened the gate");
    restore.clear_fails.store(0, SeqCst);
    engine.block_on(session.restore_retry()).unwrap();
    eventually("the retry clears", || !session.restored());
    assert!(!mark_present(&session));
}

/// C3: with Platform on and no Platform pass, the mark stays and the status
/// names the pass. The same data opened without Platform clears it once
/// Core has caught up, the loads are built and the drop-dead pass ran.
/// Opened with Platform again, the gate stays open and the hold stays in
/// effect.
#[test]
fn without_platform_the_mark_clears_on_core_and_the_loads() {
    use crate::RestoreWait::{CoreSync, PlatformPass};
    let (_dir, engine, session, id, _paid, p) = held_payment();
    *session.hub.restore.core_synced.lock().unwrap() = Some(true);
    session.hub.restore.wake.notify_one();
    eventually("only Platform's pass is waited for", || {
        waiting(&engine, &session) == Some(vec![PlatformPass])
    });
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert!(
        session.restored() && mark_present(&session),
        "the mark cleared with no Platform pass"
    );
    let session = reopen_with(&engine, session, true);
    eventually("only Core is waited for", || {
        waiting(&engine, &session) == Some(vec![CoreSync])
    });
    assert!(session.restored() && mark_present(&session));
    *session.hub.restore.core_synced.lock().unwrap() = Some(true);
    session.hub.restore.wake.notify_one();
    eventually("the mark clears without Platform", || !session.restored());
    assert!(!mark_present(&session));
    assert_eq!(waiting(&engine, &session), Some(vec![PlatformPass]));
    let session = reopen_with(&engine, session, false);
    assert!(!session.restored(), "the gate closed again");
    assert!(
        holds(&session, id).contains_key(&p.txid()),
        "the hold went with the mark"
    );
    assert_change_held(&engine, &session, id, &p, "reopened with Platform");
}

/// A mark gone outside the clear (a clear whose last step failed after the
/// remove) ends the watcher once, with what waited for the gate let run.
#[test]
fn a_mark_gone_outside_the_clear_ends_the_watcher_once() {
    use std::sync::atomic::Ordering::SeqCst;
    let (_dir, _engine, session, _id, _paid, _p) = held_payment();
    let restore = &session.hub.restore;
    std::fs::remove_file(mark_dir(&session).join("RESTORED")).unwrap();
    assert!(!session.restored());
    restore.wake.notify_one();
    eventually("the watcher lets what waited run", || {
        restore.cleared_runs.load(SeqCst) == 1
    });
    for _ in 0..5 {
        restore.wake.notify_one();
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    assert_eq!(
        restore.cleared_runs.load(SeqCst),
        1,
        "the watcher kept running"
    );
}

/// [`held_payment`] on an engine whose events are kept.
fn recorded_held_payment() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    Transaction,
    Arc<super::Recorder>,
) {
    use super::own_spend_disk_tests::{COINS, FUNDED_AT, foreign, fresh_address, funding};
    let (dir, engine, session, id, recorder) = super::engine_wallet_recorded();
    mined(&engine, &session, id, FUNDED_AT, vec![foreign()]);
    let to: Vec<_> = (0..COINS)
        .map(|_| fresh_address(&engine, &session, id))
        .collect();
    let paid = funding(7, &to, COIN);
    mempool(&engine, &session, id, &paid);
    instant_lock(&engine, &session, id, &paid);
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
    assert!(holds(&session, id).contains_key(&p.txid()));
    (dir, engine, session, id, paid, recorder)
}

fn residue_present(session: &NetworkSession) -> bool {
    mark_dir(session).join("PLATFORM_PENDING").exists()
}

/// Opened without Platform with Core caught up: the mark clears and leaves
/// the Platform residue.
fn finished_without_platform(engine: &Engine, session: Arc<NetworkSession>) -> Arc<NetworkSession> {
    let session = reopen_with(engine, session, true);
    *session.hub.restore.core_synced.lock().unwrap() = Some(true);
    session.hub.restore.wake.notify_one();
    eventually("the mark clears without Platform", || !session.restored());
    assert!(!mark_present(&session), "the mark stayed");
    assert!(residue_present(&session), "no Platform residue");
    session
}

/// C6: after a finish without Platform, a session with Platform offers a
/// Core send but refuses a Shield lease, keeps the asset-lock resume and
/// the masternode withdrawals' recheck waiting, and names Platform's pass
/// alone, until Platform is checked and its own drop-dead pass ran; then
/// the residue is gone and they run.
#[test]
fn a_finish_without_platform_holds_platform_funds_until_platform_is_checked() {
    use crate::RestoreWait::PlatformPass;
    use crate::masternodes::WithdrawalKeyChoice;
    use crate::platform::lease::resume::{Resumed, Resumer};
    use crate::platform::{FlowKind, LeaseError, WithdrawAmount};
    use dw_vault::{Credential, GrantKind, GrantPurpose};
    use std::sync::atomic::Ordering::SeqCst;
    let (_dir, engine, session, id, paid, recorder) = recorded_held_payment();
    let session = finished_without_platform(&engine, session);
    let status = engine.block_on(session.restore_status()).unwrap().unwrap();
    assert!(status.platform_only && status.platform_off, "{status:?}");
    assert_eq!(status.waiting, [PlatformPass]);
    let dead = Txid::from_byte_array([0x26; 32]);
    (session.live().unwrap().appdb)
        .set_setting(&id.to_string(), &format!("dropped:{dead}"), Some(""))
        .unwrap();
    let session = reopen_with(&engine, session, false);
    let restore = &session.hub.restore;
    assert!(!session.restored() && residue_present(&session));
    let b = OutPoint::new(paid.txid(), 1);
    let accept = Arc::new(super::own_spend_disk_tests::Accept::default());
    let (_draft, prepared) = prepare(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![b]),
        COIN / 2,
        false,
        &accept,
    );
    assert!(prepared.is_ok(), "a Core send: {:?}", prepared.err());
    let shield = engine.block_on(session.begin_lease(id, FlowKind::Shield, &[]));
    assert!(
        matches!(shield, Err(LeaseError::Restored)),
        "{:?}",
        shield.err()
    );
    let tracked = crate::platform::lease::mode_b::SessionTracked {
        session: Arc::downgrade(&session),
    };
    let txid = Txid::from_byte_array([0x31; 32]);
    assert_eq!(engine.block_on(tracked.resume(id, txid)), Resumed::NotRun);
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert_eq!(restore.recheck_runs.load(SeqCst), 0, "the recheck ran");
    let status = engine.block_on(session.restore_status()).unwrap().unwrap();
    assert!(
        status.platform_only && !status.platform_off && status.history_wallets.is_empty(),
        "{status:?}"
    );
    assert_eq!(status.waiting, [PlatformPass]);
    assert!(residue_present(&session), "the residue cleared unchecked");
    let grant = || {
        let purpose = GrantPurpose::PlatformOp {
            max_duffs: COIN,
            max_credits: COIN * 1000,
        };
        (session
            .vault()
            .authorize(purpose, Some(&id.0), Credential::None))
        .unwrap()
        .id
    };
    let dp = session.dashpay(id);
    let (g1, g2, g3, g4) = (grant(), grant(), grant(), grant());
    let codes = [
        engine
            .block_on(dp.platform_send("op".into(), g1.clone()))
            .err()
            .map(|e| e.code()),
        engine
            .block_on(dp.shielded_send("x".into(), 1000, None, g2.clone(), false))
            .err()
            .map(|e| e.code()),
        engine
            .block_on(dp.move_to_core(
                WithdrawAmount::All,
                "x".into(),
                Some("cancel-of-move-1-0-at-1-1".into()),
                g4.clone(),
            ))
            .err()
            .map(|e| e.code()),
        engine
            .block_on(session.masternodes().withdraw_credits(
                id,
                "22".repeat(32),
                WithdrawalKeyChoice::Owner,
                WithdrawAmount::Credits { credits: 100_000 },
                None,
                g3.clone(),
            ))
            .err()
            .map(|e| e.code()),
        engine
            .block_on(session.masternodes().recheck_withdrawal("33".repeat(16)))
            .err()
            .map(|e| e.code()),
    ];
    assert_eq!(codes, [Some("platform.restored"); 5]);
    for g in [&g1, &g2, &g3, &g4] {
        assert!(
            session
                .vault()
                .grant_purpose(g, GrantKind::PlatformOp, Some(&id.0))
                .is_ok(),
            "a grant was redeemed"
        );
    }
    let accept = engine.block_on(session.begin_lease(id, FlowKind::AcceptAndPay, &[]));
    assert!(
        !matches!(accept, Err(LeaseError::Restored)),
        "{:?}",
        accept.err()
    );
    drop(accept);
    let from = recorder.len();
    all_synced(&session);
    eventually("the residue clears", || !residue_present(&session));
    assert!(
        recorder.wait_for(from, |e| matches!(e, crate::EngineEvent::Sync { .. })),
        "no sync event for the panel"
    );
    assert_eq!(
        restore.pass_runs.lock().unwrap().as_slice(),
        [(id, BTreeSet::from([dead]))],
        "the clear ran its own drop-dead pass"
    );
    eventually("what waited runs", || {
        restore.cleared_runs.load(SeqCst) == 1
    });
    assert_eq!(engine.block_on(session.restore_status()).unwrap(), None);
    let shield = engine.block_on(session.begin_lease(id, FlowKind::Shield, &[]));
    assert!(
        !matches!(shield, Err(LeaseError::Restored)),
        "{:?}",
        shield.err()
    );
    assert_eq!(engine.block_on(tracked.resume(id, txid)), Resumed::Ran);
    let session = reopen_with(&engine, session, false);
    eventually("the recheck runs", || {
        session.hub.restore.recheck_runs.load(SeqCst) > 0
    });
}

/// C6: a residue that does not read, or a gate without its dir, reads
/// present.
#[cfg(unix)]
#[test]
fn an_unreadable_residue_reads_present() {
    use crate::platform::{FlowKind, LeaseError};
    assert!(crate::restore_gate::RestoreGate::default().platform_pending());
    let (_dir, engine, session, id, _paid, _p) = held_payment();
    cleared(&session);
    assert!(!session.hub.restore.platform_held());
    let dir = mark_dir(&session);
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::write(&dir, b"").unwrap();
    assert!(
        session.hub.restore.platform_pending(),
        "an unreadable residue reads absent"
    );
    let shield = engine.block_on(session.begin_lease(id, FlowKind::Shield, &[]));
    assert!(
        matches!(shield, Err(LeaseError::Restored)),
        "{:?}",
        shield.err()
    );
}

/// C6: a stop after the residue's write and before the mark's clear leaves
/// both, so a session with Platform opens with Core's gate and Platform's
/// closed.
#[test]
fn a_stop_between_the_residue_and_the_clear_leaves_both_gates_closed() {
    use crate::platform::{FlowKind, LeaseError};
    use std::sync::atomic::Ordering::SeqCst;
    let (_dir, engine, session, id, _paid, _p) = held_payment();
    let session = reopen_with(&engine, session, true);
    let restore = &session.hub.restore;
    restore.clear_fails.store(u32::MAX, SeqCst);
    *restore.backoff.lock().unwrap() = Some(std::time::Duration::from_secs(600));
    *restore.core_synced.lock().unwrap() = Some(true);
    restore.wake.notify_one();
    eventually("the clear fails", || {
        restore.clear_attempts.load(SeqCst) == 1
    });
    assert!(session.restored() && mark_present(&session));
    assert!(residue_present(&session), "no residue before the clear");
    let session = reopen_with(&engine, session, false);
    assert!(session.restored() && session.hub.restore.platform_pending());
    assert!(matches!(
        session.refuse_while_restored(),
        Err(EngineError::Restored)
    ));
    let shield = engine.block_on(session.begin_lease(id, FlowKind::Shield, &[]));
    assert!(
        matches!(shield, Err(LeaseError::Restored)),
        "{:?}",
        shield.err()
    );
}

/// C6: a residue that does not write keeps the mark; the next try writes
/// it and clears.
#[test]
fn a_residue_that_does_not_write_keeps_the_mark() {
    use std::sync::atomic::Ordering::SeqCst;
    let (_dir, engine, session, _id, _paid, _p) = held_payment();
    let session = reopen_with(&engine, session, true);
    let restore = &session.hub.restore;
    restore.residue_fails.store(1, SeqCst);
    *restore.backoff.lock().unwrap() = Some(std::time::Duration::from_secs(600));
    *restore.core_synced.lock().unwrap() = Some(true);
    restore.wake.notify_one();
    eventually("the residue's write fails", || {
        engine
            .block_on(session.restore_status())
            .unwrap()
            .is_some_and(|s| s.clear_failed)
    });
    assert!(
        session.restored() && mark_present(&session),
        "the mark cleared"
    );
    assert!(!residue_present(&session));
    engine.block_on(session.restore_retry()).unwrap();
    eventually("the retry clears", || !session.restored());
    assert!(!mark_present(&session) && residue_present(&session));
    eventually("the failure is no longer shown", || {
        engine
            .block_on(session.restore_status())
            .unwrap()
            .is_some_and(|s| s.platform_only && !s.clear_failed)
    });
}

/// In a session without Platform, a mark gone outside the clear while the
/// Platform residue stays opens Core once and ends the watcher; the
/// residue stays for a session with Platform.
#[test]
fn without_platform_a_mark_gone_beside_the_residue_ends_the_watcher_once() {
    use std::sync::atomic::Ordering::SeqCst;
    let (_dir, engine, session, _id, _paid, _p) = held_payment();
    let session = reopen_with(&engine, session, true);
    let restore = &session.hub.restore;
    let before = restore.cleared_runs.load(SeqCst);
    std::fs::write(
        mark_dir(&session).join("PLATFORM_PENDING"),
        "platform-pending\n",
    )
    .unwrap();
    std::fs::remove_file(mark_dir(&session).join("RESTORED")).unwrap();
    assert!(!session.restored() && residue_present(&session));
    restore.wake.notify_one();
    eventually("the watcher lets what waited run", || {
        restore.cleared_runs.load(SeqCst) == before + 1
    });
    for _ in 0..5 {
        restore.wake.notify_one();
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    assert_eq!(
        restore.cleared_runs.load(SeqCst),
        before + 1,
        "the watcher kept running"
    );
    assert!(
        residue_present(&session),
        "the residue cleared without Platform"
    );
}

/// Row counts of every table of app.sqlite and the dispatch journal.
fn table_rows(session: &NetworkSession) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for file in [dw_appdb::APP_DB_FILE, dw_appdb::dispatch::DISPATCH_DB_FILE] {
        let conn = rusqlite::Connection::open_with_flags(
            session.data_dir().join(file),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let tables: Vec<String> = (conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'"))
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
        for t in tables {
            let n: i64 = (conn
                .query_row(&format!("SELECT COUNT(*) FROM \"{t}\""), [], |r| r.get(0)))
            .unwrap();
            out.insert(format!("{file}/{t}"), n);
        }
    }
    out
}

/// C7: in a session without Platform, every funds flow that needs
/// Platform is `platform.unavailable` with its grant unused, no lease and
/// no row written; a Core send is offered, and Core flows' leases are not
/// refused for Platform.
#[test]
fn without_platform_every_platform_funds_flow_is_refused_up_front() {
    use crate::masternodes::WithdrawalKeyChoice;
    use crate::platform::{
        FlowKind, FundAmount, LeaseError, LockKindId, PaymentSource, RegistrationFunding,
        RegistrationRequest, WithdrawAmount,
    };
    use dw_vault::{Credential, GrantKind, GrantPurpose};
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let session = reopen_with(&engine, session, true);
    assert!(!session.restored() && !session.hub.restore.platform_held());
    std::thread::sleep(std::time::Duration::from_secs(1));
    let rows = table_rows(&session);
    let grant = || {
        let purpose = GrantPurpose::PlatformOp {
            max_duffs: COIN,
            max_credits: COIN * 1000,
        };
        (session
            .vault()
            .authorize(purpose, Some(&id.0), Credential::None))
        .unwrap()
        .id
    };
    let dp = session.dashpay(id);
    let txid = "11".repeat(32);
    let ident = "4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF".to_string();
    let request = || RegistrationRequest {
        label: "alice".into(),
        temporary_label: None,
        funding: RegistrationFunding::CoreBalance,
        initial_profile: None,
    };
    let amount = || WithdrawAmount::Credits { credits: 100_000 };
    let fund = || FundAmount::Duffs { duffs: COIN / 10 };
    let mut refused: Vec<(&str, String, String)> = Vec::new();
    macro_rules! call {
        ($name:expr, |$g:ident| $call:expr) => {{
            let $g = grant();
            let code = match engine.block_on($call) {
                Ok(_) => "ok".to_string(),
                Err(e) => e.code().to_string(),
            };
            refused.push(($name, code, $g));
        }};
    }
    call!("shielded_send", |g| dp.shielded_send(
        "x".into(),
        1000,
        None,
        g.clone(),
        false
    ));
    call!("unshield_to_core", |g| dp.unshield_to_core(
        1000,
        None,
        g.clone(),
        false
    ));
    call!("unshield_to_platform", |g| dp.unshield_to_platform(
        1000,
        None,
        g.clone(),
        false
    ));
    call!("shield_from_core", |g| dp
        .shield_from_core(fund(), g.clone()));
    call!("shield_mixed_coins", |g| dp.shield_mixed_coins(g.clone()));
    call!("resume_shield", |g| dp
        .resume_shield(txid.clone(), g.clone()));
    call!("platform_send", |g| dp
        .platform_send("op".into(), g.clone()));
    call!("move_to_core", |g| dp.move_to_core(
        amount(),
        "x".into(),
        None,
        g.clone()
    ));
    call!("platform_cancel", |g| dp.platform_cancel(
        "move-1-0".into(),
        "x".into(),
        g.clone()
    ));
    call!("platform_fund", |g| dp.platform_fund(fund(), g.clone()));
    call!("resume_platform_fund", |g| dp
        .resume_platform_fund(txid.clone(), g.clone()));
    call!("top_up", |g| dp.top_up(ident.clone(), COIN / 10, g.clone()));
    call!("resume_top_up", |g| dp
        .resume_top_up(txid.clone(), g.clone()));
    call!("withdraw", |g| dp.withdraw(
        ident.clone(),
        "x".into(),
        amount(),
        g.clone()
    ));
    call!("withdraw_to_platform", |g| dp.withdraw_to_platform(
        ident.clone(),
        "x".into(),
        amount(),
        g.clone()
    ));
    call!("top_up_from_platform", |g| dp.top_up_from_platform(
        ident.clone(),
        amount(),
        None,
        g.clone()
    ));
    call!("start_registration", |g| dp
        .start_registration(request(), g.clone()));
    call!("start_registration_past_unknown_funding", |g| dp
        .start_registration_past_unknown_funding(
            request(),
            g.clone(),
            Vec::new()
        ));
    call!("resume_registration", |g| dp
        .resume_registration("draft".into(), Some(g.clone())));
    call!("finish_asset_locks", |g| dp.finish_asset_locks(g.clone()));
    call!("register_name", |g| dp.register_name(
        ident.clone(),
        "alice".into(),
        g.clone()
    ));
    for source in [PaymentSource::Shielded, PaymentSource::Platform] {
        call!("contact_pay", |g| dp.contact_pay(
            ident.clone(),
            ident.clone(),
            1000,
            source,
            None,
            g.clone(),
            false
        ));
    }
    for kind in [LockKindId::Shielded, LockKindId::PlatformAddress] {
        call!("finish_transfer", |g| dp.finish_transfer(
            kind,
            txid.clone(),
            0,
            g.clone()
        ));
    }
    let flows = [
        FlowKind::Registration,
        FlowKind::TopUp,
        FlowKind::Withdraw,
        FlowKind::NameRegistration,
        FlowKind::AcceptAndPay,
        FlowKind::PlatformAddressMove,
        FlowKind::PlatformAddressSend,
        FlowKind::Shield,
        FlowKind::PlatformAddressFund,
        FlowKind::ShieldedSpend,
        FlowKind::ShieldDrain,
        FlowKind::MasternodeWithdraw,
    ];
    for flow in flows {
        call!("begin_flow", |g| session.begin_flow(
            id,
            flow,
            vec![g.clone()],
            None
        ));
        let g = grant();
        let lease = engine.block_on(session.begin_lease(id, flow, std::slice::from_ref(&g)));
        assert!(
            matches!(lease, Err(LeaseError::PlatformUnavailable)),
            "{flow:?}: {:?}",
            lease.err()
        );
        refused.push(("begin_lease", "platform.unavailable".into(), g));
    }
    for source in [PaymentSource::Shielded, PaymentSource::Platform] {
        call!("begin_flow", |g| session.begin_flow(
            id,
            FlowKind::ContactPayFromBalance,
            vec![g.clone()],
            Some(source)
        ));
    }
    for (name, code, g) in &refused {
        assert_eq!(code, "platform.unavailable", "{name}");
        assert!(
            session
                .vault()
                .grant_purpose(g, GrantKind::PlatformOp, Some(&id.0))
                .is_ok(),
            "{name}: the grant was redeemed"
        );
    }
    let g = grant();
    let mn = engine.block_on(session.masternodes().withdraw_credits(
        id,
        "22".repeat(32),
        WithdrawalKeyChoice::Owner,
        amount(),
        None,
        g.clone(),
    ));
    assert_eq!(
        mn.as_ref().err().map(|e| e.code()),
        Some("masternodes.platform_unavailable"),
        "{mn:?}"
    );
    assert!(
        session
            .vault()
            .grant_purpose(&g, GrantKind::PlatformOp, Some(&id.0))
            .is_ok()
    );
    assert!(session.lease_table().views().is_empty(), "a lease was made");
    assert_eq!(table_rows(&session), rows, "a row was written");
    for flow in [
        FlowKind::ContactPayment,
        FlowKind::MasternodeRegistration,
        FlowKind::KeyChange,
        FlowKind::SharedMasternode,
        FlowKind::Dissolution,
        FlowKind::Proposal,
    ] {
        let lease = engine.block_on(session.begin_lease(id, flow, &[]));
        assert!(
            !matches!(lease, Err(LeaseError::PlatformUnavailable)),
            "{flow:?}"
        );
    }
    let b = OutPoint::new(paid.txid(), 1);
    let accept = Arc::new(super::own_spend_disk_tests::Accept::default());
    let (_draft, prepared) = prepare(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![b]),
        COIN / 2,
        false,
        &accept,
    );
    assert!(prepared.is_ok(), "a Core send: {:?}", prepared.err());
}

/// A real engine and offline regtest session with a vault and no wallet.
fn engine_without_wallet() -> (tempfile::TempDir, Engine, Arc<NetworkSession>) {
    let (dir, engine) = engine_without_network();
    let session = with_vault(&engine);
    (dir, engine, session)
}

/// A real engine with no network open yet.
fn engine_without_network() -> (tempfile::TempDir, Engine) {
    use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(
        crate::EngineConfig {
            data_root: dir.path().join("data"),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..Default::default()
            },
        },
        Arc::new(super::Recorder::default()) as Arc<dyn crate::EventSink>,
    )
    .unwrap();
    (dir, engine)
}

/// Opens the offline regtest session and creates its vault.
fn with_vault(engine: &Engine) -> Arc<NetworkSession> {
    let session = super::open(engine);
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    session
}

fn import(engine: &Engine, session: &Arc<NetworkSession>) -> WalletId {
    engine
        .block_on(session.import_wallet(
            zeroize::Zeroizing::new(super::PHRASE.to_vec()),
            zeroize::Zeroizing::new(Vec::new()),
            crate::ImportOptions {
                birth_height: Some(super::BIRTH),
                ..Default::default()
            },
        ))
        .unwrap()
}

/// Q107: a restored network with no wallet (a recovery's fresh folder)
/// keeps its mark with Core, Platform and the passes done, and names the
/// wallet it waits for; once a wallet is added in the session, its load
/// builds the holds and the mark clears.
#[test]
fn the_mark_stays_while_the_network_has_no_wallet() {
    use crate::RestoreWait::Wallet;
    let (_dir, engine, session) = engine_without_wallet();
    let session = restored_with(&engine, session, "");
    all_synced(&session);
    eventually("only a wallet is waited for", || {
        waiting(&engine, &session) == Some(vec![Wallet])
    });
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert!(
        session.restored() && mark_present(&session),
        "the mark cleared with no wallet"
    );
    let id = import(&engine, &session);
    eventually("the mark clears", || !session.restored());
    assert!(!mark_present(&session));
    assert!((session.hub.history).read_wallet(&id, |h| h.restore_holds_built));
}

/// Q107 with C6: the Platform residue of a network with no wallet stays
/// once Platform is checked, and names the wallet; once a wallet is
/// loaded, it clears.
#[test]
fn the_platform_residue_stays_while_the_network_has_no_wallet() {
    use crate::RestoreWait::Wallet;
    let (_dir, engine, session) = engine_without_wallet();
    let dir = mark_dir(&session);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("PLATFORM_PENDING"), "platform-pending\n").unwrap();
    let session = reopen(&engine, session);
    assert!(!session.restored() && residue_present(&session));
    all_synced(&session);
    eventually("only a wallet is waited for", || {
        waiting(&engine, &session) == Some(vec![Wallet])
    });
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert!(
        residue_present(&session),
        "the residue cleared with no wallet"
    );
    import(&engine, &session);
    session.hub.restore.wake.notify_one();
    eventually("the residue clears", || !residue_present(&session));
}

/// Q107 C1 S1: the mark `recover` writes in a fresh folder (no pending
/// lines, `from <left>`) stays with no wallet and clears, in the same
/// session, once a wallet restored from its phrase has loaded and synced.
#[test]
fn a_recovered_folders_mark_clears_once_a_restored_wallet_has_synced() {
    use crate::RestoreWait::Wallet;
    let (_dir, engine) = engine_without_network();
    let root = engine.config().data_root.clone();
    let folder = crate::network_folders::recover(&root, &crate::DashNetwork::Regtest).unwrap();
    let mark = std::fs::read_to_string(
        (folder.join(dw_store_upgrade::UPGRADE_BACKUPS_DIR)).join(dw_store_upgrade::RESTORED_MARK),
    )
    .unwrap();
    let lines: Vec<&str> = mark.lines().collect();
    assert_eq!(lines.len(), 3, "{mark}");
    assert!(
        lines[2].starts_with("restored ") && lines[2].ends_with("Z from regtest"),
        "{mark}"
    );
    let session = with_vault(&engine);
    assert_eq!(session.data_dir(), folder.as_path());
    assert!(session.restored(), "the gate is on in the recovered folder");
    all_synced(&session);
    eventually("only a wallet is waited for", || {
        waiting(&engine, &session) == Some(vec![Wallet])
    });
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert!(
        session.restored() && mark_present(&session),
        "the mark cleared with no wallet"
    );
    let id = import(&engine, &session);
    assert!((session.hub.history).read_wallet(&id, |h| h.restore_holds_built));
    eventually("the mark clears", || !session.restored());
    assert!(!mark_present(&session) && !residue_present(&session));
}

/// A recovery's fresh folder (a mark with no pending lines, no wallet):
/// a wallet restored from its phrase in the session builds its holds at
/// its load there, and the mark clears with no reopen.
#[test]
fn a_wallet_restored_in_session_clears_a_recover_mark() {
    use crate::RestoreWait::Wallet;
    let (_dir, engine, session) = engine_without_wallet();
    std::fs::create_dir_all(mark_dir(&session)).unwrap();
    std::fs::write(
        mark_dir(&session).join("RESTORED"),
        "dw-store-upgrade restored\nformat 1\nrestored 2026-11-25T00:00:00Z from before\n",
    )
    .unwrap();
    let session = reopen(&engine, session);
    assert!(session.restored());
    all_synced(&session);
    eventually("only a wallet is waited for", || {
        waiting(&engine, &session) == Some(vec![Wallet])
    });
    let id = import(&engine, &session);
    assert!((session.hub.history).read_wallet(&id, |h| h.restore_holds_built));
    eventually("the mark clears", || !session.restored());
    assert!(!mark_present(&session) && !residue_present(&session));
}

fn watch_only_xpub() -> key_wallet::bip32::ExtendedPubKey {
    use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &[21; 64]).unwrap();
    let path: DerivationPath = "m/44'/1'/0'".parse().unwrap();
    ExtendedPubKey::from_priv(&master.derive_priv(&path).unwrap())
}

/// While the mark holds, a wallet is added only from its phrase: a
/// watch-only add is refused with `RestoreOnly`, the mark stays, and
/// nothing of the wallet is written (no restore hold, no Platform row).
#[test]
fn a_watch_only_wallet_is_refused_while_the_mark_holds() {
    let (_dir, engine, session) = engine_without_wallet();
    let session = restored_with(&engine, session, "");
    all_synced(&session);
    let xpub = watch_only_xpub();
    let id = crate::multiwallet::watch_only_wallet_id(&xpub, session.network.core_network());
    let added = engine.block_on(session.import_watch_only(xpub.to_string(), Default::default()));
    assert!(matches!(added, Err(EngineError::RestoreOnly)), "{added:?}");
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert!(
        session.restored() && mark_present(&session),
        "the mark cleared"
    );
    let holds = (session.live().unwrap().appdb)
        .settings_with_prefix_lossy(&id.to_string(), crate::restore_gate::RESTORE_HOLD_PREFIX)
        .unwrap();
    assert!(holds.is_empty(), "{holds:?}");
    assert!(!platform_row(&session, id));
}

/// A Platform pass noted for a watch-only wallet before it is added does
/// not count for it: once the residue clears, it keeps its Platform row.
#[test]
fn a_pass_before_a_watch_only_wallet_is_added_does_not_check_it() {
    let (_dir, engine, session) = engine_without_wallet();
    let dir = mark_dir(&session);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("PLATFORM_PENDING"), "platform-pending\n").unwrap();
    let session = reopen(&engine, session);
    let xpub = watch_only_xpub();
    let id = crate::multiwallet::watch_only_wallet_id(&xpub, session.network.core_network());
    let restore = &session.hub.restore;
    *restore.core_synced.lock().unwrap() = Some(true);
    restore.note_platform_pass();
    restore.note_shielded_pass();
    restore.note_wallet_platform_pass([id]);
    restore.note_wallet_shielded_pass([id]);
    let added = engine
        .block_on(session.import_watch_only(xpub.to_string(), Default::default()))
        .unwrap();
    assert_eq!(added, id);
    eventually("the residue clears", || !residue_present(&session));
    assert!(
        platform_row(&session, id),
        "a pass before its import checked it"
    );
}

#[test]
fn a_pass_before_attaching_keys_does_not_release_the_wallet() {
    use crate::platform::{FlowKind, LeaseError};

    let (_dir, engine, session) = engine_without_wallet();
    let id = import(&engine, &session);
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    let dir = mark_dir(&session);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("PLATFORM_PENDING"), "platform-pending\n").unwrap();
    let session = reopen(&engine, session);
    assert!(!session.vault().has_wallet_secret(&id.0));
    let restore = &session.hub.restore;
    *restore.core_synced.lock().unwrap() = Some(false);
    restore.note_platform_pass();
    restore.note_shielded_pass();
    restore.note_wallet_platform_pass([id]);
    restore.note_wallet_shielded_pass([id]);

    assert_eq!(import(&engine, &session), id);
    *restore.core_synced.lock().unwrap() = Some(true);
    restore.wake.notify_one();
    eventually("the network's residue clears", || {
        !residue_present(&session)
    });
    let status = engine.block_on(session.restore_status()).unwrap().unwrap();
    assert!(status.platform_only);
    assert_eq!(status.waiting, [crate::RestoreWait::PlatformPass]);
    let shield = || engine.block_on(session.begin_lease(id, FlowKind::Shield, &[]));
    assert!(matches!(shield(), Err(LeaseError::Restored)));

    restore.note_wallet_platform_pass([id]);
    restore.note_wallet_shielded_pass([id]);
    eventually("the post-attach pass clears the wallet's row", || {
        !platform_row(&session, id)
    });
    assert!(!matches!(shield(), Err(LeaseError::Restored)));
    assert_eq!(engine.block_on(session.restore_status()).unwrap(), None);
}

/// A Platform pass noted for a wallet before it is added in the session
/// does not count for it: once the residue clears, the added wallet keeps
/// its Platform row until a pass after its registration.
#[test]
fn a_pass_before_a_wallet_is_added_does_not_check_it() {
    let id = {
        let (_dir, engine, session) = engine_without_wallet();
        import(&engine, &session)
    };
    let (_dir, engine, session) = engine_without_wallet();
    let dir = mark_dir(&session);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("PLATFORM_PENDING"), "platform-pending\n").unwrap();
    let session = reopen(&engine, session);
    let restore = &session.hub.restore;
    *restore.core_synced.lock().unwrap() = Some(true);
    restore.note_platform_pass();
    restore.note_shielded_pass();
    restore.note_wallet_platform_pass([id]);
    restore.note_wallet_shielded_pass([id]);
    assert_eq!(import(&engine, &session), id);
    eventually("the residue clears", || !residue_present(&session));
    assert!(
        platform_row(&session, id),
        "a pass before its import checked it"
    );
}

/// Whether `wallet` has its Platform row in app.sqlite.
fn platform_row(session: &NetworkSession, wallet: WalletId) -> bool {
    (session.live().unwrap().appdb)
        .setting(
            &wallet.to_string(),
            crate::restore_gate::RESTORE_PLATFORM_PENDING,
        )
        .unwrap()
        .is_some()
}

/// After a finish without Platform, a session with Platform opened with
/// a second wallet closed; the returned session is open, its residue
/// present, the second wallet not loaded.
fn residue_with_a_closed_wallet() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    WalletId,
) {
    let (dir, engine, session, id, _paid, _p) = held_payment();
    let other = engine
        .block_on(
            session.import_wallet(
                zeroize::Zeroizing::new(
                    b"legal winner thank year wave sausage worth useful legal winner thank yellow"
                        .to_vec(),
                ),
                zeroize::Zeroizing::new(Vec::new()),
                crate::ImportOptions {
                    birth_height: Some(super::BIRTH),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
    let session = finished_without_platform(&engine, session);
    engine
        .block_on(session.set_load_on_startup(other, false))
        .unwrap();
    let session = reopen_with(&engine, session, false);
    assert!(session.live().unwrap().store.is_unloaded(&other.0));
    assert!(residue_present(&session) && !session.restored());
    (dir, engine, session, id, other)
}

/// Option (b): a wallet closed when Platform's check cleared the residue
/// keeps its Platform funds held by its own row; a later session with
/// Platform refuses its Shield lease and its Platform send until a pass
/// after its load (one before it does not count) and its drop-dead pass
/// ran; the other wallet is not held.
#[test]
fn a_wallet_closed_at_the_platform_check_is_held_until_checked_after_its_load() {
    use crate::platform::{FlowKind, LeaseError};
    use dw_vault::{Credential, GrantPurpose};
    use std::sync::atomic::Ordering::SeqCst;
    let (_dir, engine, session, id, other) = residue_with_a_closed_wallet();
    all_synced(&session);
    eventually("the residue clears", || !residue_present(&session));
    assert!(
        platform_row(&session, other),
        "the closed wallet has no row"
    );
    assert!(!platform_row(&session, id), "the checked wallet has a row");
    let session = reopen_with(&engine, session, false);
    let restore = &session.hub.restore;
    eventually("the watcher checks the closed wallet's row", || {
        restore.row_scan_attempts.load(SeqCst) > 0
    });
    std::thread::sleep(std::time::Duration::from_millis(2_500));
    assert_eq!(restore.row_scan_attempts.load(SeqCst), 1);
    *restore.core_synced.lock().unwrap() = Some(true);
    let dead = Txid::from_byte_array([0x27; 32]);
    (session.live().unwrap().appdb)
        .set_setting(&other.to_string(), &format!("dropped:{dead}"), Some(""))
        .unwrap();
    restore.note_wallet_platform_pass([other]);
    restore.note_wallet_shielded_pass([other]);
    engine.block_on(session.load_wallet(other)).unwrap();
    let shield = |w| engine.block_on(session.begin_lease(w, FlowKind::Shield, &[]));
    assert!(
        matches!(shield(other), Err(LeaseError::Restored)),
        "{:?}",
        shield(other).err()
    );
    assert!(!matches!(shield(id), Err(LeaseError::Restored)));
    let grant = (session.vault())
        .authorize(
            GrantPurpose::PlatformOp {
                max_duffs: COIN,
                max_credits: COIN * 1000,
            },
            Some(&other.0),
            Credential::None,
        )
        .unwrap()
        .id;
    let send = engine.block_on(session.dashpay(other).platform_send("op".into(), grant));
    assert_eq!(send.err().map(|e| e.code()), Some("platform.restored"));
    let status = engine.block_on(session.restore_status()).unwrap().unwrap();
    assert!(status.platform_only && !status.platform_off, "{status:?}");
    assert_eq!(status.waiting, [crate::RestoreWait::PlatformPass]);
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert!(
        platform_row(&session, other),
        "a pass before the load counted"
    );
    assert!(restore.pass_runs.lock().unwrap().is_empty());
    restore.note_wallet_platform_pass([other]);
    restore.note_wallet_shielded_pass([other]);
    eventually("the wallet's row clears", || !platform_row(&session, other));
    assert_eq!(
        restore.pass_runs.lock().unwrap().as_slice(),
        [(other, BTreeSet::from([dead]))],
        "the clear ran the wallet's drop-dead pass"
    );
    assert!(!matches!(shield(other), Err(LeaseError::Restored)));
    assert_eq!(engine.block_on(session.restore_status()).unwrap(), None);
}

/// A wallet's Platform row that does not read holds its Platform funds.
#[test]
fn an_unreadable_platform_row_reads_present() {
    use crate::platform::{FlowKind, LeaseError};
    let (_dir, engine, session, id, _paid) = wallet_funded_unmined();
    let restore = &session.hub.restore;
    assert!(!session.wallet_platform_held(id));
    restore
        .rows_unreadable
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let shield = || engine.block_on(session.begin_lease(id, FlowKind::Shield, &[]));
    assert!(matches!(shield(), Err(LeaseError::Restored)));
    let status = engine.block_on(session.restore_status()).unwrap().unwrap();
    assert!(status.platform_only, "{status:?}");
    restore
        .rows_unreadable
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert!(!matches!(shield(), Err(LeaseError::Restored)));
    assert_eq!(engine.block_on(session.restore_status()).unwrap(), None);
}

/// The rows are written before the residue goes: a failed write keeps the
/// residue without rows, a failed removal leaves both, and the next clear
/// removes the residue and keeps the row.
#[test]
fn the_platform_rows_are_written_before_the_residue_goes() {
    use std::sync::atomic::Ordering::SeqCst;
    let (_dir, engine, session, _id, other) = residue_with_a_closed_wallet();
    let failed = || {
        engine
            .block_on(session.restore_status())
            .unwrap()
            .is_some_and(|s| s.clear_failed)
    };
    let restore = &session.hub.restore;
    *restore.backoff.lock().unwrap() = Some(std::time::Duration::from_secs(600));
    let path = session.data_dir().join(dw_appdb::APP_DB_FILE);
    dw_appdb::fault::arm(
        &path,
        crate::restore_gate::PLATFORM_ROWS_FAULT,
        dw_appdb::fault::Stage::Before,
    );
    all_synced(&session);
    eventually("the rows' write fails", &failed);
    dw_appdb::fault::disarm(&path);
    assert!(residue_present(&session) && !platform_row(&session, other));
    restore.residue_remove_fails.store(1, SeqCst);
    engine.block_on(session.restore_retry()).unwrap();
    eventually("the row is written", || platform_row(&session, other));
    eventually("the removal fails", || {
        restore.residue_remove_fails.load(SeqCst) == 0 && failed()
    });
    assert!(residue_present(&session), "the residue went");
    engine.block_on(session.restore_retry()).unwrap();
    eventually("the residue clears", || !residue_present(&session));
    assert!(platform_row(&session, other), "the row went with it");
}

/// P1: the wallets that get a row come from the network's wallet list; a
/// list that does not read keeps the residue, retried after the backoff.
#[test]
fn an_unreadable_wallet_list_keeps_the_residue() {
    use std::sync::atomic::Ordering::SeqCst;
    let (_dir, engine, session, _id, other) = residue_with_a_closed_wallet();
    let restore = &session.hub.restore;
    *restore.backoff.lock().unwrap() = Some(std::time::Duration::from_secs(600));
    restore.wallet_list_unreadable.store(true, SeqCst);
    all_synced(&session);
    eventually("the clear fails", || {
        engine
            .block_on(session.restore_status())
            .unwrap()
            .is_some_and(|s| s.clear_failed)
    });
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert!(residue_present(&session), "the residue cleared");
    restore.wallet_list_unreadable.store(false, SeqCst);
    engine.block_on(session.restore_retry()).unwrap();
    eventually("the residue clears", || !residue_present(&session));
    assert!(platform_row(&session, other));
}

/// A mark in RECOVER's format (no pending list) clears at all-Synced with
/// a wallet loaded, in a session with Platform: the wallet closed then
/// keeps its Platform row, and no residue is left.
#[test]
fn a_recover_mark_clears_and_leaves_a_closed_wallet_its_platform_row() {
    let (_dir, engine, session, id, _paid) = wallet_funded_unmined();
    let other = engine
        .block_on(session.create_wallet(12))
        .unwrap()
        .wallet_id;
    engine
        .block_on(session.set_load_on_startup(other, false))
        .unwrap();
    std::fs::create_dir_all(mark_dir(&session)).unwrap();
    std::fs::write(
        mark_dir(&session).join("RESTORED"),
        "dw-store-upgrade restored\nformat 1\nrestored 2026-11-25T00:00:00Z from before\n",
    )
    .unwrap();
    let session = reopen(&engine, session);
    assert!(session.restored() && session.live().unwrap().store.is_unloaded(&other.0));
    cleared(&session);
    assert!(!residue_present(&session));
    assert!(platform_row(&session, other) && !platform_row(&session, id));
}

/// A wallet loaded after Platform's passes, still unchecked when they
/// clear the residue, keeps its Platform funds held by its own row while
/// it stays loaded, until a pass of it; the checked wallet is not held.
#[test]
fn a_wallet_loaded_after_the_passes_keeps_its_platform_row_until_checked() {
    use crate::platform::{FlowKind, LeaseError};
    let (_dir, engine, session, id, other) = residue_with_a_closed_wallet();
    let restore = &session.hub.restore;
    *restore.core_synced.lock().unwrap() = Some(true);
    engine.block_on(session.load_wallet(other)).unwrap();
    restore.note_wallet_platform_pass([id]);
    restore.note_wallet_shielded_pass([id]);
    restore.note_platform_pass();
    restore.note_shielded_pass();
    eventually("the residue clears", || !residue_present(&session));
    assert!(platform_row(&session, other), "the late wallet has no row");
    assert!(!platform_row(&session, id));
    let shield = |w| engine.block_on(session.begin_lease(w, FlowKind::Shield, &[]));
    assert!(matches!(shield(other), Err(LeaseError::Restored)));
    assert!(!matches!(shield(id), Err(LeaseError::Restored)));
    let status = engine.block_on(session.restore_status()).unwrap().unwrap();
    assert!(status.platform_only, "{status:?}");
    restore.note_wallet_platform_pass([other]);
    restore.note_wallet_shielded_pass([other]);
    eventually("the late wallet's row clears", || {
        !platform_row(&session, other)
    });
    assert!(!matches!(shield(other), Err(LeaseError::Restored)));
    assert_eq!(engine.block_on(session.restore_status()).unwrap(), None);
}
