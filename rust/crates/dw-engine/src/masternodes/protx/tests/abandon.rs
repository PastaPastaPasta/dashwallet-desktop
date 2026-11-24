//! MG-10 §1.10's D49 way out of a ProTx that never mines: once Activity
//! (or Remove unconfirmed) abandons a `sent` registration's or key change's
//! transaction, the operation reads `abandoned` and holds none of its coins
//! (they unlock and are selectable again), also after a restart, and is
//! never resent. Core may still mine it, so a key change keeps its
//! payout-change barrier (MG-13 P16) and holds the signing of another
//! update until it is `conflicted`. Before the abandon nothing changes, and
//! a confirmed operation settles as it did.

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
        self.seen_with(tx, TransactionContext::Mempool);
    }

    /// [`Self::seen_in_mempool`] in `context`.
    fn seen_with(&self, tx: &Transaction, context: TransactionContext) {
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
                .check_core_transaction(&t, context, &mut wallet, true, true)
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
    f.assert_released_by_the_abandon(&op, funding);
    assert_eq!(kind(f.state(&op)), "abandoned");
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

    /// The state P16 clears `op`'s barrier by.
    fn op_barrier_state(&self, op: &ProTxOperation) -> Option<ProTxOperationState> {
        let row = self.rows().into_iter().find(|r| r.id == op.id).unwrap();
        (self.engine)
            .block_on(self.session.barrier_operation_state(self.wallet, &row))
            .unwrap()
    }

    /// The abandon itself released `op`'s fee inputs: read from the
    /// database, before anything that settles again.
    fn assert_released_by_the_abandon(&self, op: &ProTxOperation, coin: OutPoint) {
        let row = self.rows().into_iter().find(|r| r.id == op.id).unwrap();
        assert_eq!(row.fee_inputs, "[]", "not released by the abandon");
        assert!(
            !self.locked().contains(&coin),
            "not unlocked by the abandon"
        );
    }

    /// A block holds a payment of `coin` to elsewhere at `height`.
    fn spent_in_a_block(&self, coin: OutPoint, height: u32) {
        let rival = Transaction {
            version: 2,
            lock_time: 0,
            input: vec![dashcore::TxIn {
                previous_output: coin,
                ..Default::default()
            }],
            output: vec![TxOut {
                value: COIN / 2,
                script_pubkey: dashcore::ScriptBuf::new(),
            }],
            special_transaction_payload: None,
        };
        self.confirm(rival, height);
        self.spend(coin);
    }

    fn assert_abandoned_pending(&self, e: &MasternodesError, op: &ProTxOperation) {
        assert!(
            matches!(e, MasternodesError::UpdateAbandonedPending { operation } if *operation == op.id),
            "{e:?}"
        );
        assert_eq!(e.code(), "masternodes.update_abandoned_pending");
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
/// on it. Activity's abandon releases the fee coin: it is selectable and
/// unlocks, and the next preview goes through. Core may still mine it, so
/// its payout-change barrier stays (MG-13 P16), and signing another update
/// is refused, even paid from another coin, until one of its coins is
/// spent in a block (it is `conflicted`).
#[test]
fn mg10_an_abandoned_key_change_frees_its_coins_and_holds_the_next_update_until_conflicted() {
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
    f.assert_released_by_the_abandon(&op, fee_coin);
    assert_eq!(kind(f.state(&op)), "abandoned");
    assert_eq!(
        f.barrier_ops(),
        std::slice::from_ref(&op.id),
        "the barrier went"
    );
    assert_eq!(
        f.op_barrier_state(&op),
        Some(ProTxOperationState::Abandoned)
    );
    f.credit(2, COIN);
    assert!(f.selectable(fee_coin), "{:?}", f.held());
    f.unlock(fee_coin).unwrap();
    f.assert_resend_refused(&op);
    f.preview_update(key_change(&hash, 6)).unwrap();
    // Paid from another coin, it would share no input with the abandoned one.
    // Refused before its grant is redeemed, a coin locked or anything signed.
    f.credit(3, COIN);
    let handed = f.lib.sent().len();
    let locked = f.locked();
    let grant = f.grant(COIN);
    let e = f.update(key_change(&hash, 6), grant.clone()).unwrap_err();
    f.assert_abandoned_pending(&e, &op);
    assert_eq!(f.lib.sent().len(), handed, "handed off");
    assert_eq!(f.locked(), locked);

    f.spent_in_a_block(fee_coin, 60);
    assert_eq!(f.state(&op), ProTxOperationState::Conflicted);
    assert_eq!(
        f.op_barrier_state(&op),
        Some(ProTxOperationState::Conflicted)
    );
    let next = f.update(key_change(&hash, 6), grant).unwrap();
    assert_eq!(next.state, ProTxOperationState::Sent);
}

/// The abandoned key change is mined anyway: it reads `confirmed`, and with
/// the synced list past its block the next update is sent.
#[test]
fn mg10_an_abandoned_key_change_mined_anyway_lifts_its_hold() {
    let f = fx(Library::Accept);
    let (_, reg) = f.registered();
    let hash = reg.pro_tx_hash.clone().unwrap();
    let (op, tx, fee_coin) = f.sent_key_change(&hash);
    f.abandon(tx.txid());
    f.credit(3, COIN);
    let e = f.update(key_change(&hash, 6), f.grant(COIN)).unwrap_err();
    f.assert_abandoned_pending(&e, &op);

    f.spend(fee_coin);
    f.confirm(tx.clone(), 60);
    assert_eq!(f.state(&op), ProTxOperationState::Confirmed { height: 60 });
    let next = f.update(key_change(&hash, 6), f.grant(COIN)).unwrap();
    assert_eq!(next.state, ProTxOperationState::Sent);
}

/// The abandoned key change is mined above the synced list while an edit
/// that keeps the voting key is being checked: the list still shows the old
/// key, so the edit would restore it. One read of the masternode's
/// operations judges both holds, and the edit is refused; the next one
/// waits for the list to reach the block.
#[test]
fn mg10_an_abandoned_key_change_mined_during_an_edits_checks_still_holds_it() {
    let f = fx(Library::Accept);
    let (_, reg) = f.registered();
    let hash = reg.pro_tx_hash.clone().unwrap();
    let (op, tx, fee_coin) = f.sent_key_change(&hash);
    f.abandon(tx.txid());
    f.credit(3, COIN);
    let (handed, rows) = (f.lib.sent().len(), f.rows().len());
    let read = Arc::new(crate::coins::TestPause::default());
    let _read = crate::held_writes::ReleaseOnDrop(Arc::clone(&read));
    *f.session.hub.pause_ops_read.lock().unwrap() = Some(Arc::clone(&read));
    let edit = {
        let (mn, id, req, grant) = (f.mn(), f.wallet, keep(&hash), f.grant(COIN));
        (f.session.rt).spawn(async move { mn.update_registrar(id, req, grant).await })
    };
    assert!(
        read.wait_reached(std::time::Duration::from_secs(60)),
        "the edit never read the operations"
    );
    f.spend(fee_coin);
    f.confirm(tx.clone(), 150);
    read.release.notify_one();
    let e = f.engine.block_on(edit).unwrap().unwrap_err();
    f.assert_abandoned_pending(&e, &op);
    assert_eq!(f.lib.sent().len(), handed, "handed off");
    assert_eq!(f.rows().len(), rows, "signed and saved");

    assert_eq!(f.state(&op), ProTxOperationState::Confirmed { height: 150 });
    let e = f.update(keep(&hash), f.grant(COIN)).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::UpdatePending { operation } if *operation == op.id),
        "{e:?}"
    );
}

/// A replacement signed and not sent before the change it replaces was
/// abandoned (an earlier version signed one): its resend is refused while
/// the abandoned change may still be mined, and nothing reaches the
/// broadcaster.
#[test]
fn mg10_a_signed_replacements_resend_waits_for_the_abandoned_change() {
    let f = fx(Library::Accept);
    let (_, reg) = f.registered();
    let hash = reg.pro_tx_hash.clone().unwrap();
    let (op, tx, _) = f.sent_key_change(&hash);
    f.abandon(tx.txid());
    let mut replacement = f.rows().into_iter().find(|r| r.id == op.id).unwrap();
    replacement.id = "e".repeat(32);
    replacement.txid = Some("cd".repeat(32));
    replacement.created_at += 1;
    replacement.sent_at = None;
    replacement.last_error = Some("send.no_peers".into());
    replacement.fee_inputs = "[]".into();
    (f.db())
        .insert_masternode_op_durable(&f.wallet.to_string(), &replacement, &[], 1)
        .unwrap();
    let replacement = (f.operations().into_iter())
        .find(|o| o.id == "e".repeat(32))
        .unwrap();
    assert_eq!(replacement.state, ProTxOperationState::NotSent);
    let handed = f.lib.sent().len();
    let e = f.resend(&replacement).unwrap_err();
    f.assert_abandoned_pending(&e, &op);
    assert_eq!(f.lib.sent().len(), handed, "handed off");
}

/// The release is one transaction: when it fails before its commit, the
/// fee coin stays locked and `fee_inputs` stays. Unlock then settles again:
/// while that fails too it refuses with the failure, not as reserved; once
/// it succeeds it releases both. The barrier stays throughout (P16).
#[test]
fn mg10_an_abandoned_key_changes_release_is_one_transaction() {
    let f = fx(Library::Accept);
    let (_, reg) = f.registered();
    let hash = reg.pro_tx_hash.clone().unwrap();
    let (op, tx, fee_coin) = f.sent_key_change(&hash);
    let path = f.session.data_dir().join(dw_appdb::APP_DB_FILE);
    let stage = dw_appdb::fault::Stage::Before;
    dw_appdb::fault::arm(&path, dw_appdb::fault::MN_OP_ABANDON, stage);
    f.abandon(tx.txid());
    assert!(f.locked().contains(&fee_coin));
    assert_eq!(f.barrier_ops(), std::slice::from_ref(&op.id));
    let row = f.rows().into_iter().find(|r| r.id == op.id).unwrap();
    assert_eq!(row.fee_inputs, format!("[\"{fee_coin}\"]"));

    f.credit(2, COIN);
    dw_appdb::fault::arm(&path, dw_appdb::fault::MN_OP_ABANDON, stage);
    match f.unlock(fee_coin) {
        Err(crate::EngineError::Storage(m)) => assert!(m.contains("mn_op.before_abandon"), "{m}"),
        other => panic!("{other:?}"),
    }
    dw_appdb::fault::disarm(&path);
    f.unlock(fee_coin).unwrap();
    assert!(f.selectable(fee_coin), "{:?}", f.held());
    assert_eq!(f.barrier_ops(), std::slice::from_ref(&op.id));
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
/// change reads `abandoned`, its fee coin unlocks and is selectable, a
/// resend is refused, and its barrier stays (P16).
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
    assert_eq!(f.barrier_ops(), std::slice::from_ref(&op.id));
    f.unlock(fee_coin).unwrap();
    assert!(f.selectable(fee_coin), "{:?}", f.held());
    f.assert_resend_refused(&op);
}

/// An InstantSend lock seen after the abandon outranks it, as the key change
/// will be mined: it reads `sent` again, holds the next update of its
/// masternode, and a withdrawal waits on it through the history as well as
/// its barrier. A resend is still refused: the abandon's mark stands.
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
    assert_eq!(f.barrier_ops(), std::slice::from_ref(&op.id));
    f.assert_resend_refused(&op);
}

/// A key change whose one hand-off definitely failed (`not_sent`) and that
/// reached the wallet's history anyway, abandoned in Activity: its resend is
/// refused and nothing reaches the broadcaster; Discard, for bytes the
/// wallet never sent, still ends it.
#[test]
fn mg10_an_abandoned_not_sent_operation_is_never_resent_but_discards() {
    let f = fx(Library::Accept);
    let (_, reg) = f.registered();
    let hash = reg.pro_tx_hash.clone().unwrap();
    *f.lib.mode.lock().unwrap() = Library::NoPeers;
    let op = f.update(key_change(&hash, 5), f.grant(COIN)).unwrap();
    assert_eq!(op.state, ProTxOperationState::NotSent);
    *f.lib.mode.lock().unwrap() = Library::Accept;
    let tx = f.lib.sent().pop().unwrap();
    f.seen_in_mempool(&tx);
    f.abandon(tx.txid());
    assert_eq!(f.state(&op), ProTxOperationState::NotSent);
    f.assert_resend_refused(&op);
    (f.engine)
        .block_on(f.mn().discard_operation(f.wallet, op.id.clone()))
        .unwrap();
    assert!(f.rows().iter().all(|r| r.id != op.id), "the row stays");
}

/// Tools ▸ Repair ▸ Remove unconfirmed transactions abandons through the
/// same finishing step: the sent registration's coins are released by it.
#[test]
fn mg10_remove_unconfirmed_frees_a_sent_registrations_coins() {
    let f = fx(Library::Accept);
    let (op, _, funding) = f.sent_registration();
    f.run_spv();
    let removed = (f.engine)
        .block_on(f.session.drop_unconfirmed(Some(f.wallet)))
        .unwrap();
    assert!(removed >= 1, "{removed}");
    f.assert_released_by_the_abandon(&op, funding);
    assert_eq!(kind(f.state(&op)), "abandoned");
    f.assert_resend_refused(&op);
}

/// Remove unconfirmed keeps what is InstantSend-locked, as its confirmation
/// says (`abandon_refusal`): a locked registration is not abandoned, reads
/// `sent` and keeps its coins.
#[test]
fn mg10_remove_unconfirmed_keeps_an_instantsend_locked_registration() {
    let f = fx(Library::Accept);
    let (op, tx, funding) = f.sent_registration();
    f.seen_with(&tx, TransactionContext::InstantSend(Default::default()));
    f.run_spv();
    (f.engine)
        .block_on(f.session.drop_unconfirmed(Some(f.wallet)))
        .unwrap();
    let marks = f
        .engine
        .block_on(f.session.abandoned_txids(f.wallet))
        .unwrap();
    assert!(!marks.contains(&tx.txid()), "abandoned");
    assert_eq!(f.state(&op), ProTxOperationState::Sent);
    f.assert_unlock_refused(funding);
    assert!(!f.selectable(funding), "{:?}", f.held());
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
