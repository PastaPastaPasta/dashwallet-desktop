//! MG-10 §1.10's D49 way out of a ProTx that never mines: once Activity
//! abandons a `sent` registration's or key change's transaction, the
//! operation reads `abandoned`, holds none of its coins (they unlock and
//! are selectable again) nor its payout-change barrier, holds no later
//! update and is never resent, also after a restart. Before the abandon
//! nothing changes, and a confirmed operation settles as it did.

use key_wallet::transaction_checking::WalletTransactionChecker;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;

use super::*;

/// `unlock_outpoints`' refusal of a coin a masternode row holds.
const RESERVED: &str = "the coin is reserved by a masternode transaction";

/// `state`'s snake-case name, as `OperationState{state}` carries it.
fn kind(state: ProTxOperationState) -> String {
    serde_json::to_value(state).unwrap()["kind"]
        .as_str()
        .unwrap()
        .to_owned()
}

impl Fx {
    /// The wallet records `tx` in the mempool, as dash-spv records a local
    /// broadcast: key-wallet's record, then the history's entry.
    fn seen_in_mempool(&self, tx: &Transaction) {
        let (manager, id, t) = (
            Arc::clone(&self.session.live().unwrap().manager),
            self.wallet,
            tx.clone(),
        );
        let record = self.engine.block_on(async move {
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let (wallet, info) = wm.get_wallet_and_info_mut(&id.0).unwrap();
            let mut wallet = wallet.clone();
            info.core_wallet
                .check_core_transaction(&t, TransactionContext::Mempool, &mut wallet, true, true)
                .await;
            (info.core_wallet.transaction_history().into_iter())
                .find(|r| r.txid == t.txid())
                .cloned()
        });
        let record = record.expect("key-wallet records the transaction");
        (self.session.hub.history).with_wallet(self.wallet, |h| h.upsert(&record, None));
    }

    /// Activity's abandon of `txid` (its checks pass: SPV isn't running
    /// here).
    fn abandon(&self, txid: Txid) {
        let (s, id) = (Arc::clone(&self.session), self.wallet);
        self.engine
            .block_on(self.session.on_runtime(async move {
                let manager = s.manager()?;
                s.abandon_inner(&manager, id, txid).await
            }))
            .unwrap();
    }

    fn unlock(&self, coin: OutPoint) -> Result<(), crate::EngineError> {
        self.engine
            .block_on(self.session.unlock_outpoints(self.wallet, vec![coin]))
    }

    fn resend(&self, op: &ProTxOperation) -> Result<ProTxOperation, MasternodesError> {
        self.engine.block_on(self.mn().resend_operation(
            self.wallet,
            op.id.clone(),
            self.grant(COIN),
        ))
    }

    fn state(&self, op: &ProTxOperation) -> ProTxOperationState {
        let ops = self.operations();
        ops.into_iter().find(|o| o.id == op.id).unwrap().state
    }

    /// A sent `FundNew` registration the wallet recorded in the mempool:
    /// the operation, its transaction and its funding coin (seed 1).
    fn sent_registration(&self) -> (ProTxOperation, Transaction, OutPoint) {
        let funding = self.credit(1, MASTERNODE_COLLATERAL + COIN);
        let op = self
            .register(request(CollateralChoice::FundNew), self.grant(COIN))
            .unwrap();
        assert_eq!(op.state, ProTxOperationState::Sent);
        let tx = self.lib.sent().pop().unwrap();
        assert_eq!(tx.input[0].previous_output, funding);
        self.seen_in_mempool(&tx);
        (op, tx, funding)
    }

    /// The coin is offered to a build: neither locked nor reserved.
    fn selectable(&self, coin: OutPoint) -> bool {
        self.held().contains(&(coin, false, false))
    }

    /// Unlock refuses `coin` as held by a masternode row, and it stays
    /// locked.
    fn assert_unlock_refused(&self, coin: OutPoint) {
        match self.unlock(coin) {
            Err(crate::EngineError::InvalidArgument(m)) => assert_eq!(m, RESERVED),
            other => panic!("{other:?}"),
        }
        assert!(self.locked().contains(&coin), "unlocked");
    }

    /// A resend of `op` is refused as `abandoned`, and nothing is handed
    /// off.
    fn assert_resend_refused(&self, op: &ProTxOperation) {
        let handed = self.lib.sent().len();
        match self.resend(op) {
            Err(MasternodesError::OperationState { state }) => assert_eq!(state, "abandoned"),
            other => panic!("{other:?}"),
        }
        assert_eq!(self.lib.sent().len(), handed, "handed off again");
    }
}

/// Before Activity's abandon, the sent registration keeps its coins: Unlock
/// refuses its funding coin and the follower releases nothing.
#[test]
fn mg10_before_its_abandon_a_sent_registration_keeps_its_coins() {
    let f = fx(Library::Accept);
    let (op, _, funding) = f.sent_registration();
    f.credit(1, MASTERNODE_COLLATERAL + COIN);
    f.follow();
    assert_eq!(f.state(&op), ProTxOperationState::Sent);
    f.assert_unlock_refused(funding);
    assert!(!f.selectable(funding));
    assert_eq!(f.rows()[0].fee_inputs, format!("[\"{funding}\"]"));
}

/// D49: Activity abandons the sent registration's transaction, and the
/// operation reads `abandoned`. Its funding coin (the collateral's) is no
/// longer held: once the rescan returns it, Unlock succeeds and a build
/// may select it.
#[test]
fn mg10_an_abandoned_registration_frees_its_coins() {
    let f = fx(Library::Accept);
    let (op, tx, funding) = f.sent_registration();
    f.abandon(tx.txid());
    assert_eq!(kind(f.state(&op)), "abandoned");
    assert_eq!(f.rows()[0].fee_inputs, "[]");
    f.credit(1, MASTERNODE_COLLATERAL + COIN);
    f.unlock(funding).unwrap();
    assert!(!f.locked().contains(&funding));
    assert!(f.selectable(funding), "{:?}", f.held());
    // Core may still mine it: its service stays claimed (D51).
    let e = f
        .register(request(CollateralChoice::FundNew), f.grant(COIN))
        .unwrap_err();
    assert!(protx_dup(&e, &op.id), "{e:?}");
}

/// D49's last step: a spend of the abandoned registration's released coin,
/// buried in a block, makes it `conflicted` (a block outranks the abandon),
/// and its service is free again.
#[test]
fn mg10_a_spend_of_an_abandoned_registrations_coin_makes_it_conflicted() {
    let f = fx(Library::Accept);
    let (op, tx, funding) = f.sent_registration();
    f.abandon(tx.txid());
    let rival = Transaction {
        version: 2,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: funding,
            ..Default::default()
        }],
        output: vec![TxOut {
            value: COIN,
            script_pubkey: dashcore::ScriptBuf::new(),
        }],
        special_transaction_payload: None,
    };
    f.confirm(rival, 60);
    f.spend(funding);
    f.tip(66);
    assert_eq!(f.state(&op), ProTxOperationState::Conflicted);
    f.credit(2, MASTERNODE_COLLATERAL + COIN);
    let next = f
        .register(request(CollateralChoice::FundNew), f.grant(COIN))
        .unwrap();
    assert_eq!(next.state, ProTxOperationState::Sent);
}

impl Fx {
    /// A sent key change of the registered masternode that changes its
    /// payout (so it has a payout-change barrier), recorded in the mempool:
    /// the operation, its transaction and its fee coin (seed 2).
    fn sent_key_change(&self, hash: &str) -> (ProTxOperation, Transaction, OutPoint) {
        let op = self.update(key_change(hash, 5), self.grant(COIN)).unwrap();
        assert_eq!(op.state, ProTxOperationState::Sent);
        let tx = self.lib.sent().pop().unwrap();
        self.seen_in_mempool(&tx);
        let fee_coin = tx.input[0].previous_output;
        (op, tx, fee_coin)
    }

    fn barrier_ops(&self) -> Vec<String> {
        let barriers = self.db().payout_barriers().unwrap();
        barriers.into_iter().map(|b| b.barrier.op_id).collect()
    }
}

/// A key change setting voting key `index`, paid to a new address.
fn key_change(hash: &str, index: u32) -> UpdateRegistrarRequest {
    UpdateRegistrarRequest {
        voting: KeepOr::Set(VotingChoice::Keychain { index }),
        ..keep(hash)
    }
}

/// #438: a key change (ProUpRegTx) that changes the payout. Before the
/// abandon its fee coin is held and the next update of the masternode waits
/// on it. Activity's abandon releases the fee coin and the payout-change
/// barrier together: the coin is selectable and unlocks, and the next
/// preview and update go through.
#[test]
fn mg10_an_abandoned_key_change_frees_its_coins_and_the_next_update() {
    let f = fx(Library::Accept);
    let (_, reg) = f.registered();
    let hash = reg.pro_tx_hash.clone().unwrap();
    let (op, tx, fee_coin) = f.sent_key_change(&hash);
    assert_eq!(f.barrier_ops(), std::slice::from_ref(&op.id));
    f.assert_unlock_refused(fee_coin);
    let e = f.preview_update(key_change(&hash, 6)).unwrap_err();
    assert_eq!(e.code(), "masternodes.update_pending");
    let e = f.update(key_change(&hash, 6), f.grant(COIN)).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::UpdatePending { operation } if *operation == op.id),
        "{e:?}"
    );

    f.abandon(tx.txid());
    assert_eq!(kind(f.state(&op)), "abandoned");
    assert!(f.barrier_ops().is_empty(), "the barrier stays");
    f.credit(2, COIN);
    assert!(f.selectable(fee_coin), "{:?}", f.held());
    f.unlock(fee_coin).unwrap();
    f.assert_resend_refused(&op);
    f.preview_update(key_change(&hash, 6)).unwrap();
    let next = f.update(key_change(&hash, 6), f.grant(COIN)).unwrap();
    assert_eq!(next.state, ProTxOperationState::Sent);
}

/// The release is one transaction: when it fails before its commit, the
/// fee coin stays locked, `fee_inputs` and the barrier stay; Unlock's
/// settling then releases all three.
#[test]
fn mg10_an_abandoned_key_changes_release_is_one_transaction() {
    let f = fx(Library::Accept);
    let (_, reg) = f.registered();
    let hash = reg.pro_tx_hash.clone().unwrap();
    let (op, tx, fee_coin) = f.sent_key_change(&hash);
    let path = f.session.data_dir().join(dw_appdb::APP_DB_FILE);
    let stage = dw_appdb::fault::Stage::Before;
    dw_appdb::fault::arm(&path, dw_appdb::fault::MN_BARRIER_ABANDON, stage);
    f.abandon(tx.txid());
    dw_appdb::fault::disarm(&path);
    assert!(f.locked().contains(&fee_coin));
    assert_eq!(f.barrier_ops(), std::slice::from_ref(&op.id));
    let row = f.rows().into_iter().find(|r| r.id == op.id).unwrap();
    assert_eq!(row.fee_inputs, format!("[\"{fee_coin}\"]"));

    f.credit(2, COIN);
    f.unlock(fee_coin).unwrap();
    assert!(f.selectable(fee_coin), "{:?}", f.held());
    assert!(f.barrier_ops().is_empty());
}

/// Once its coins are released, the abandoned registration is terminal:
/// Send again is refused with `operation_state{state: "abandoned"}` and
/// nothing goes out, though its coin is back and unlocked; Discard still
/// refuses it.
#[test]
fn mg10_an_abandoned_registration_is_never_resent() {
    let f = fx(Library::Accept);
    let (op, tx, funding) = f.sent_registration();
    f.abandon(tx.txid());
    f.credit(1, MASTERNODE_COLLATERAL + COIN);
    f.assert_resend_refused(&op);
    f.unlock(funding).unwrap();
    f.assert_resend_refused(&op);
    let discard = f
        .engine
        .block_on(f.mn().discard_operation(f.wallet, op.id.clone()));
    match discard {
        Err(MasternodesError::OperationState { state }) => assert_eq!(state, "abandoned"),
        other => panic!("{other:?}"),
    }
    assert_eq!(f.rows().len(), 1, "the row went");
    assert!(f.selectable(funding));
}

/// The abandon's release fails (the follower's settling step), the app
/// restarts, and the coins are free all the same: the abandoned mark is
/// durable, so after the restart the operation reads `abandoned`, Unlock
/// succeeds, the coin is selectable and a resend is refused.
#[test]
fn mg10_an_abandon_frees_the_coins_after_a_restart() {
    let mut f = fx(Library::Accept);
    let (op, tx, funding) = f.sent_registration();
    f.crash_at(Some(CrashPoint::Settle));
    f.abandon(tx.txid());
    assert!(f.locked().contains(&funding), "released before the restart");

    f.restart(&[(1, MASTERNODE_COLLATERAL + COIN)]);
    f.unlock(funding).unwrap();
    assert!(f.selectable(funding), "{:?}", f.held());
    assert_eq!(kind(f.state(&op)), "abandoned");
    assert_eq!(f.rows()[0].fee_inputs, "[]");
    f.assert_resend_refused(&op);
}

/// A confirmed registration settles as it did: its inputs released, its new
/// collateral kept locked; also one Activity abandoned before it was mined
/// anyway (a block outranks the abandon).
#[test]
fn mg10_a_confirmed_registration_settles_as_before() {
    for abandoned in [false, true] {
        let f = fx(Library::Accept);
        let (op, tx, funding) = f.sent_registration();
        let vout = payload(&tx).collateral_outpoint.vout;
        let collateral = OutPoint::new(tx.txid(), vout);
        if abandoned {
            f.abandon(tx.txid());
        }
        f.spend(funding);
        f.hold(collateral, tx.output[vout as usize].clone());
        f.confirm(tx.clone(), 7);
        assert_eq!(
            f.state(&op),
            ProTxOperationState::Confirmed { height: 7 },
            "{abandoned}"
        );
        assert_eq!(f.locked(), HashSet::from([collateral]), "{abandoned}");
        assert_eq!(f.rows()[0].fee_inputs, "[]", "{abandoned}");
        let resent = f.resend(&op).unwrap_err();
        assert!(
            matches!(&resent, MasternodesError::OperationState { state } if state == "confirmed"),
            "{abandoned}: {resent:?}"
        );
    }
}

impl Fx {
    fn try_abandon(&self, txid: Txid) -> Result<BTreeSet<Txid>, crate::EngineError> {
        let (s, id) = (Arc::clone(&self.session), self.wallet);
        self.engine.block_on(self.session.on_runtime(async move {
            let manager = s.manager()?;
            s.abandon_inner(&manager, id, txid).await
        }))
    }
}

/// The abandon stores its marks and its wallet sweep fails, and the app
/// restarts: the load finishes the abandon from the marks, and the key
/// change reads `abandoned`, its barrier is gone, its fee coin unlocks and
/// is selectable, and a resend is refused.
#[test]
fn mg10_an_abandon_whose_sweep_failed_frees_the_coins_after_a_restart() {
    let mut f = fx(Library::Accept);
    let (_, reg) = f.registered();
    let hash = reg.pro_tx_hash.clone().unwrap();
    let (op, tx, fee_coin) = f.sent_key_change(&hash);
    (f.session.hub.fail_abandon_sweep).store(true, std::sync::atomic::Ordering::SeqCst);
    let failed = f.try_abandon(tx.txid());
    assert!(
        matches!(failed, Err(crate::EngineError::Storage(_))),
        "{failed:?}"
    );
    assert_eq!(f.barrier_ops(), std::slice::from_ref(&op.id));

    f.restart(&[(2, COIN)]);
    assert_eq!(kind(f.state(&op)), "abandoned");
    assert!(f.barrier_ops().is_empty(), "the barrier stays");
    f.unlock(fee_coin).unwrap();
    assert!(f.selectable(fee_coin), "{:?}", f.held());
    f.assert_resend_refused(&op);
}

/// An InstantSend lock seen after the abandon outranks it, as the key change
/// will be mined: it reads `sent` again, holds the next update of its
/// masternode, and a withdrawal waits on it through the history.
#[test]
fn mg10_an_instantsend_lock_after_the_abandon_makes_the_key_change_sent_again() {
    let f = fx(Library::Accept);
    let (reg_tx, reg) = f.registered();
    let hash = reg.pro_tx_hash.clone().unwrap();
    let pro_tx_hash = reg_tx.txid().to_byte_array();
    let (op, tx, _) = f.sent_key_change(&hash);
    f.abandon(tx.txid());
    assert_eq!(kind(f.state(&op)), "abandoned");
    assert!(f.session.withdrawal_payout_changes(&pro_tx_hash).is_empty());

    let instant = TransactionContext::InstantSend(Default::default());
    f.block_event(vec![tx_record(tx.clone(), instant)]);
    assert_eq!(f.state(&op), ProTxOperationState::Sent);
    let e = f.preview_update(key_change(&hash, 6)).unwrap_err();
    assert_eq!(e.code(), "masternodes.update_pending");
    let e = f.update(key_change(&hash, 6), f.grant(COIN)).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::UpdatePending { operation } if *operation == op.id),
        "{e:?}"
    );
    assert_eq!(
        f.session.withdrawal_payout_changes(&pro_tx_hash),
        vec![None]
    );
}

/// An abandon that lands while a resend waits for the coin fence, past its
/// state check: the resend reads the state again under the fence, is
/// refused as `abandoned`, and hands nothing off.
#[test]
fn mg10_an_abandon_during_a_resend_stops_its_hand_off() {
    let f = fx(Library::Accept);
    let (op, tx, _) = f.sent_registration();
    let handed = f.lib.sent().len();
    let fence = f.engine.block_on(f.session.coin_fence(f.wallet)).unwrap();
    let resend = {
        let (mn, id, operation, grant) = (f.mn(), f.wallet, op.id.clone(), f.grant(COIN));
        f.session
            .rt
            .spawn(async move { mn.resend_operation(id, operation, grant).await })
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while f.session.coin_fences.waiting(f.wallet) == 0 {
        assert!(
            !resend.is_finished(),
            "the resend never waited for the fence"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "the resend never waited for the fence"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    f.abandon(tx.txid());
    drop(fence);
    match f.engine.block_on(resend).unwrap() {
        Err(MasternodesError::OperationState { state }) => assert_eq!(state, "abandoned"),
        other => panic!("{other:?}"),
    }
    assert_eq!(f.lib.sent().len(), handed, "handed off again");
}
