//! Repair's "Resolve stuck funding" of a step whose own asset lock the
//! wallet recorded as a payment that never settled
//! (`platform/lease/repair.rs`, "The lock the wallet recorded"): the first
//! Resolve abandons that payment and signs nothing, and once the rescan
//! gives its coins back, the next one sends them to the wallet.

use std::collections::BTreeSet;
use std::sync::Arc;

use dashcore::{OutPoint, Transaction, Txid};
use dw_vault::{Credential, GrantPurpose};
use platform_wallet::key_wallet_manager::WalletInterface;

use super::own_spend_disk_tests::{
    Accept, COIN, FUNDED_AT, TIP, assert_funding_refused, chain_lock, foreign, own_scripts, send,
    spendable, start_spv, wait_stored, wallet_funded_unmined,
};
use super::{block_with, mempool, mined};
use crate::platform::lease::{FlowOutcome, FundingKey, StoredLock, StuckFundingRefusal};
use crate::platform::{DispatchState, FlowKind};
use crate::send::CoinSource;
use crate::session::NetworkSession;
use crate::{Engine, EngineError, WalletId};

/// A registration's funding step whose call signed `lock` and failed: its
/// marker stands, and the wallet tracks `lock` as `Broadcast` with no
/// proof. Returns the step.
fn stuck_on(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    lock: &Transaction,
) -> String {
    use dw_appdb::{SqlValue, TableRows};
    let w = id.to_string();
    let draft = engine
        .block_on(session.appdb_op(move |db| {
            let text = |t: &str| SqlValue::Text(t.to_owned());
            let columns = [
                "wallet_id",
                "identity_index",
                "label",
                "funding",
                "phase",
                "created_at",
                "updated_at",
            ];
            db.import_wallet_rows(
                &w,
                &[TableRows {
                    table: "dp_registration".into(),
                    columns: columns.map(String::from).to_vec(),
                    rows: vec![vec![
                        text(&w),
                        SqlValue::Integer(0),
                        text("alice"),
                        text("{}"),
                        text("draft"),
                        SqlValue::Integer(1),
                        SqlValue::Integer(1),
                    ]],
                }],
            )?;
            Ok(db.registration_identity_indices(&w)?[0].0)
        }))
        .unwrap();
    let step = format!("registration/{draft}/funding");
    let grant = (session.vault())
        .authorize(
            GrantPurpose::PlatformOp {
                max_duffs: 10_000_000,
                max_credits: 0,
            },
            Some(&id.0),
            Credential::None,
        )
        .unwrap()
        .id;
    let lease =
        (engine.block_on(session.begin_lease(id, FlowKind::Registration, &[grant]))).unwrap();
    let fence = engine.block_on(session.coin_fence(id)).unwrap();
    let key = FundingKey {
        funding_type: platform_wallet::AssetLockFundingType::IdentityRegistration,
        identity_index: 0,
        account: 0,
    };
    let permit = engine
        .block_on(session.funding_call_permit(
            &lease,
            "create_funded_asset_lock_proof",
            1_000_000,
            step.clone(),
            key,
            fence,
        ))
        .unwrap();
    let sighash = [0x42; 32];
    permit.note_signed(sighash);
    session.leases.record_signed_locks(
        id,
        &[StoredLock {
            txid: lock.txid(),
            inputs: lock.input.iter().map(|i| i.previous_output).collect(),
            sighashes: Some(vec![sighash]),
            built: true,
        }],
    );
    assert_eq!(permit.finish(false), FlowOutcome::MaybeSent);
    let row = platform_wallet::TrackedAssetLock {
        operation_id: None,
        shielded_recipient: None,
        out_point: OutPoint::new(lock.txid(), 0),
        transaction: lock.clone(),
        account_index: 0,
        funding_type: platform_wallet::AssetLockFundingType::IdentityRegistration,
        identity_index: 0,
        amount: lock.output[0].value,
        status: platform_wallet::AssetLockStatus::Broadcast,
        proof: None,
    };
    let s = Arc::clone(session);
    (engine.block_on(session.on_runtime(async move {
        let w = s.wallet(&id).await?;
        w.state_mut()
            .await
            .tracked_asset_locks
            .insert(row.out_point, row);
        Ok(())
    })))
    .unwrap();
    refresh(engine, session, id);
    step
}

fn refresh(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId) {
    let leases = Arc::clone(&session.leases);
    let refreshed = session.on_runtime(async move { Ok(leases.refresh_funding(id).await) });
    assert!(engine.block_on(refreshed).unwrap());
}

fn status(session: &NetworkSession, id: WalletId, step: &str) -> Option<DispatchState> {
    session.leases.funding_status(id, step)
}

/// The wallet's change of `tx`, duffs.
fn change(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId, tx: &Transaction) -> u64 {
    let own = own_scripts(engine, session, id);
    (tx.output.iter())
        .filter(|o| own.contains(&o.script_pubkey))
        .map(|o| o.value)
        .sum()
}

/// A wallet whose stuck registration funding's lock, A's payment, the
/// wallet recorded unsettled, with SPV started. Returns the step, the lock,
/// A and A's payment, not mined yet.
fn recorded_stuck_lock() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    String,
    Transaction,
    OutPoint,
    Transaction,
) {
    let (dir, engine, session, id, paid) = wallet_funded_unmined();
    let a = OutPoint::new(paid.txid(), 0);
    let lock = send(
        &engine,
        &session,
        id,
        CoinSource::Outpoints(vec![a]),
        COIN / 2,
        false,
    );
    mempool(&engine, &session, id, &lock);
    wait_stored(&session, id, &[lock.txid()]);
    start_spv(&engine, &session);
    let step = stuck_on(&engine, &session, id, &lock);
    assert!(
        matches!(
            status(&session, id, &step),
            Some(DispatchState::MaybeSent | DispatchState::WillBeSent)
        ),
        "{:?}",
        status(&session, id, &step)
    );
    (dir, engine, session, id, step, lock, a, paid)
}

/// Resolve, as Tools ▸ Repair runs it: its self-spend, if it builds one,
/// handed to the test instead of the network.
fn resolve(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    step: &str,
) -> Result<Transaction, EngineError> {
    let draft = engine.block_on(session.stuck_funding_draft_as(id, step, true))?;
    let seen = Arc::new(Accept::default());
    *draft.broadcaster.lock().unwrap() = Some(seen.clone());
    let grant = (session.vault())
        .authorize(
            GrantPurpose::Spend { max_duffs: 10_000 },
            Some(&id.0),
            Credential::None,
        )
        .unwrap()
        .id;
    engine.block_on(session.send_self_spend(&draft, step, grant))?;
    let tx = seen.0.lock().unwrap().pop().expect("broadcast");
    Ok(tx)
}

fn phase_one(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId, step: &str, lock: Txid) {
    match resolve(engine, session, id, step) {
        Err(EngineError::StuckFunding(StuckFundingRefusal::LockAbandoned(t))) => {
            assert_eq!(t, lock)
        }
        other => panic!("not phase 1: {other:?}"),
    }
}

fn stopped(session: &NetworkSession, engine: &Engine, txid: Txid) -> bool {
    let _ = engine;
    crate::tx_actions::rebroadcast_stop_asked(session, txid)
}

/// The pinned dash-spv keeps no stop (`tx_actions::stop_rebroadcast`), so
/// the asks recorded so far stand for what a restarted SPV no longer holds.
fn spv_forgot_its_stops(session: &NetworkSession) {
    session.hub.stopped_rebroadcasts.lock().unwrap().clear();
}

/// The rescan the abandon scheduled reaches the tip, finding A's payment
/// `paid` mined on its way: the holds go.
fn rescan_back(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId, paid: &Transaction) {
    for height in FUNDED_AT..=TIP {
        let txs = if height == FUNDED_AT + 1 {
            vec![paid.clone()]
        } else {
            vec![foreign()]
        };
        mined(engine, session, id, height, txs);
    }
    assert!(session.hub.rescan_held(&id).is_empty(), "still held");
}

/// Phase 1: Resolve abandons the lock the wallet recorded, refused
/// `LockAbandoned`. The lock's rebroadcast stops, its coin is neither
/// counted nor offered while it waits for the rescan, nothing is signed,
/// and Activity still refuses an Abandon of the lock.
#[test]
fn resolve_abandons_the_recorded_lock_and_signs_nothing() {
    let (_dir, engine, session, id, step, lock, a, _paid) = recorded_stuck_lock();
    assert_funding_refused(&engine, &session, id, lock.txid());
    let before = super::balances(&session, id);

    phase_one(&engine, &session, id, &step, lock.txid());
    assert!(
        (session.hub.history.get(&id, &lock.txid())).is_some_and(|e| e.is_abandoned()),
        "the lock is abandoned"
    );
    assert!(stopped(&session, &engine, lock.txid()), "not stopped");
    assert_eq!(session.hub.rescan_held(&id), BTreeSet::from([a]));
    assert!(!spendable(&engine, &session, id, a), "A is offered");
    assert_eq!(
        super::balances(&session, id).total,
        before.total - change(&engine, &session, id, &lock),
        "A is counted, or the lock's change"
    );
    assert!(
        session.leases.call_step(id, &step).self_spends.is_empty(),
        "a self-spend was recorded"
    );
    assert!(
        matches!(
            status(&session, id, &step),
            Some(DispatchState::MaybeSent | DispatchState::WillBeSent)
        ),
        "the step moved"
    );
    // Before the rescan, Resolve again builds nothing.
    phase_one(&engine, &session, id, &step, lock.txid());
}

#[test]
fn reopening_during_repair_keeps_unrelated_funding_available() {
    use platform_wallet::changeset::{
        AssetLockChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
    };

    let (_dir, engine, session, id, step, lock, a, _paid) = recorded_stuck_lock();
    let outpoint = OutPoint::new(lock.txid(), 0);
    let wallet = engine.block_on(session.wallet(&id)).unwrap();
    let tracked = engine.block_on(wallet.asset_locks().list_tracked_locks());
    let row = tracked
        .iter()
        .find(|row| row.out_point == outpoint)
        .unwrap();
    let mut changes = AssetLockChangeSet::default();
    changes.asset_locks.insert(outpoint, row.into());
    let persister = session.live().unwrap().persister;
    persister
        .store(
            id.0,
            PlatformWalletChangeSet {
                asset_locks: Some(changes),
                ..Default::default()
            },
        )
        .unwrap();
    persister.flush(id.0).unwrap();
    drop(wallet);
    phase_one(&engine, &session, id, &step, lock.txid());

    let session = super::reopen(&engine, session);
    assert!(engine.block_on(session.abandon_marked(id, lock.txid())));
    let wallet = engine.block_on(session.wallet(&id)).unwrap();
    assert!(
        engine
            .block_on(wallet.asset_locks().list_tracked_locks())
            .iter()
            .any(|row| row.out_point == outpoint)
    );
    start_spv(&engine, &session);
    refresh(&engine, &session, id);
    super::own_spend_disk_tests::eventually("an unrelated funding step is available", || {
        session
            .leases
            .funding_block(id, "topup/0-1/funding")
            .is_none()
    });
    assert!(session.leases.funding_block(id, &step).is_some());
    assert!(!spendable(&engine, &session, id, a));
}

/// Phase 2: once the rescan gives A back, Resolve's self-spend spends it;
/// once that is ChainLocked the step reads not sent.
#[test]
fn resolve_after_the_rescan_sends_the_locks_coins_back() {
    let (_dir, engine, session, id, step, lock, a, paid) = recorded_stuck_lock();
    phase_one(&engine, &session, id, &step, lock.txid());
    rescan_back(&engine, &session, id, &paid);

    let spend = resolve(&engine, &session, id, &step).unwrap();
    assert!(
        spend.input.iter().any(|i| i.previous_output == a),
        "{spend:?}"
    );
    assert!(!spendable(&engine, &session, id, a), "A is offered");

    let block = block_with(vec![spend.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![spend.clone()]);
    chain_lock(&engine, &session, id, TIP + 1, block.block_hash());
    refresh(&engine, &session, id);
    assert_eq!(status(&session, id, &step), Some(DispatchState::NotSent));
    let closed = resolve(&engine, &session, id, &step);
    assert!(
        matches!(
            closed,
            Err(EngineError::StuckFunding(StuckFundingRefusal::NotStuck(
                Some(DispatchState::NotSent)
            )))
        ),
        "{closed:?}"
    );
}

/// A resume of the lock between the phases: the catch-up resends it,
/// which lifts its stop (here SPV's restart: the pinned dash-spv keeps no
/// stop, [`spv_forgot_its_stops`]), and
/// the wallet sees it again unconfirmed. It stays abandoned and A is
/// counted once; the next Resolve stops its rebroadcast again rather than
/// wedging, and after the rescan sends A back.
#[test]
fn a_lock_resent_between_the_phases_is_stopped_again() {
    let (_dir, engine, session, id, step, lock, a, paid) = recorded_stuck_lock();
    phase_one(&engine, &session, id, &step, lock.txid());
    let abandoned = super::balances(&session, id).total;

    let tx = lock.clone();
    super::deliver(&engine, &session, id, move |wm| {
        Box::pin(async move {
            wm.process_mempool_transaction(&tx, None).await;
        })
    });
    let manager = session.manager().unwrap();
    engine.block_on(async move { manager.spv().stop().await.unwrap() });
    start_spv(&engine, &session);
    spv_forgot_its_stops(&session);
    assert!(
        (session.hub.history.get(&id, &lock.txid())).is_some_and(|e| e.is_abandoned()),
        "the lock is no longer abandoned"
    );
    assert_eq!(super::balances(&session, id).total, abandoned);

    phase_one(&engine, &session, id, &step, lock.txid());
    assert!(stopped(&session, &engine, lock.txid()), "not stopped again");
    assert_eq!(super::balances(&session, id).total, abandoned);
    assert_eq!(session.hub.rescan_held(&id), BTreeSet::from([a]));

    rescan_back(&engine, &session, id, &paid);
    let spend = resolve(&engine, &session, id, &step).unwrap();
    assert!(
        spend.input.iter().any(|i| i.previous_output == a),
        "{spend:?}"
    );
}

/// The abandoned lock confirms after phase 1 after all: no later Resolve
/// sends anything back. Mined, it waits for its ChainLock as any unsettled
/// spend (an abandon would refuse it); ChainLocked, the step is not stuck.
#[test]
fn a_lock_that_confirms_after_phase_one_is_never_sent_back() {
    let (_dir, engine, session, id, step, lock, _a, paid) = recorded_stuck_lock();
    phase_one(&engine, &session, id, &step, lock.txid());
    rescan_back(&engine, &session, id, &paid);
    let block = block_with(vec![lock.clone()]);
    mined(&engine, &session, id, TIP + 1, vec![lock.clone()]);
    refresh(&engine, &session, id);
    let preview = engine.block_on(session.stuck_funding_draft(id, &step));
    assert!(matches!(
        preview,
        Err(EngineError::StuckFunding(StuckFundingRefusal::OwnLockUnfinalized(t))) if t == lock.txid()
    ));
    let mined_only = resolve(&engine, &session, id, &step);
    assert!(
        matches!(
            &mined_only,
            Err(EngineError::StuckFunding(StuckFundingRefusal::SpendUnsettled(t))) if *t == lock.txid()
        ),
        "{mined_only:?}"
    );

    chain_lock(&engine, &session, id, TIP + 1, block.block_hash());
    refresh(&engine, &session, id);
    let locked = resolve(&engine, &session, id, &step);
    assert!(
        matches!(
            locked,
            Err(EngineError::StuckFunding(
                StuckFundingRefusal::NotStuck(_) | StuckFundingRefusal::LockInputsSpent(_)
            ))
        ),
        "{locked:?}"
    );
    assert!(
        session.leases.call_step(id, &step).self_spends.is_empty(),
        "a self-spend was recorded"
    );
}
