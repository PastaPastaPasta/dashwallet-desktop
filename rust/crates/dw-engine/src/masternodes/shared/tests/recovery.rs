//! A shared session's recovery (MG-10 §4.4 EP 4, T4): what the read lists,
//! each offered action passing its call's guards at the listed charge, the
//! calls it leaves out refused, and the abandon spend's bound, refusal and
//! replay (change 4), through the real hand-off on the test network.

use std::collections::BTreeMap;

use dashcore::blockdata::script::{Builder, Instruction, PushBytes};
use dashcore::secp256k1::{Message, SecretKey, ecdsa};
use dashcore::{Address, PublicKey, ScriptBuf};
use dw_protx::shared::SpecialTx;
use key_wallet::transaction_checking::TransactionContext;

use super::super::recovery::{SIGNER_RUNS, spend_fee};
use super::*;
use crate::masternodes::{
    AbandonSpendState, BlockedReason, HandoffErrorOutcome, HandoffTx, RecoveryAction, SettledBy,
    Settlement, SharedRecovery,
};

const OVERDUE_SECS: u64 = 3 * 60 * 60;

impl Fx {
    /// The SPV client as the recovery read sees it: running, and peers.
    fn spv(&self, running: bool, peers: u32) {
        *self.seam.spv.lock().unwrap() = Some((running, peers));
    }

    fn recovery(&self, w: usize, id: &str) -> SharedRecovery {
        self.session_of(w, id).recovery
    }

    fn abandon_at(&self, w: usize, id: &str, max: u64) -> Result<SharedSession, MasternodesError> {
        let grant = self.grant(w, max);
        self.run(
            self.mn()
                .abandon_shared_session(self.wallets[w], id.to_string(), grant),
        )
    }

    fn resend_abandon(
        &self,
        w: usize,
        id: &str,
        max: u64,
    ) -> Result<SharedSession, MasternodesError> {
        let grant = self.grant(w, max);
        self.run(
            self.mn()
                .resend_shared_abandon(self.wallets[w], id.to_string(), grant),
        )
    }

    /// The value and address of a coin wallet `w` holds.
    fn coin_of(&self, w: usize, outpoint: OutPoint) -> (u64, Address) {
        self.held_coin(w, outpoint).expect("a held coin")
    }

    fn held_coin(&self, w: usize, outpoint: OutPoint) -> Option<(u64, Address)> {
        let (session, id) = (Arc::clone(&self.session), self.wallets[w]);
        let inner = Arc::clone(&session);
        self.run(session.on_runtime(async move {
            let wallet = inner.wallet(&id).await?;
            let state = wallet.state_mut().await;
            let utxos = &state.core_wallet.accounts.standard_bip44_accounts[&0].utxos;
            Ok(utxos.get(&outpoint).map(|u| (u.value(), u.address.clone())))
        }))
        .unwrap()
    }

    /// Wallet `w`'s coin `outpoint` moved to an address no account of the
    /// wallet derives: it has no ownership path.
    fn orphan(&self, w: usize, outpoint: OutPoint) {
        let (session, id) = (Arc::clone(&self.session), self.wallets[w]);
        let inner = Arc::clone(&session);
        self.run(session.on_runtime(async move {
            let wallet = inner.wallet(&id).await?;
            let mut state = wallet.state_mut().await;
            let utxo = (state.core_wallet.accounts.standard_bip44_accounts)
                .get_mut(&0)
                .expect("BIP44 account 0")
                .utxos
                .get_mut(&outpoint)
                .expect("a held coin");
            let script =
                dashcore::ScriptBuf::new_p2pkh(&dashcore::PubkeyHash::from_byte_array([7; 20]));
            utxo.address = Address::from_script(&script, dashcore::Network::Regtest).unwrap();
            utxo.txout.script_pubkey = script;
            Ok(())
        }))
        .unwrap();
    }

    /// `tx` recorded unconfirmed in wallet `w`'s history, spending the
    /// wallet's coins (read before they leave it), then those coins gone.
    fn record(&self, w: usize, tx: &Transaction) {
        let own_inputs: BTreeMap<u32, (u64, Address)> = (tx.input.iter().enumerate())
            .filter_map(|(i, input)| Some((i as u32, self.held_coin(w, input.previous_output)?)))
            .collect();
        self.session.hub.history.with_wallet(self.wallets[w], |h| {
            h.txs.insert(
                tx.txid(),
                crate::history::TxEntry {
                    tx: tx.clone(),
                    context: TransactionContext::Mempool,
                    instant_locked: false,
                    first_seen: None,
                    own_inputs,
                    abandoned: false,
                },
            )
        });
        for input in &tx.input {
            self.spend(w, input.previous_output);
        }
        // The wallet saw the coins spent: no claim holds them now.
        let (_, unspent) = self.run(self.session.op_view(self.wallets[w])).unwrap();
        self.session
            .spends
            .retain_unspent(&self.wallets[w], &unspent);
    }

    /// Edits wallet `w`'s history entry of `txid`.
    fn history_edit(&self, w: usize, txid: Txid, edit: impl FnOnce(&mut crate::history::TxEntry)) {
        self.session.hub.history.with_wallet(self.wallets[w], |h| {
            edit(h.txs.get_mut(&txid).expect("a recorded transaction"))
        });
    }

    /// The row as an older build left it after the upgrade's backfill (the
    /// migration's own shape): the latest abandon spend `txid` with no
    /// bytes, a legacy `maybe_sent` entry signed at `signed_at`, the bound.
    fn legacy(&self, w: usize, id: &str, txid: Txid, signed_at: u64) {
        let log = serde_json::json!([{
            "txid": txid.to_string(), "fee": null, "inputs": [], "signed_at": signed_at,
            "outcome": "maybe_sent", "code": null, "legacy": true,
        }]);
        let conn = rusqlite::Connection::open(self.session.data_dir().join(dw_appdb::APP_DB_FILE))
            .unwrap();
        let n = conn
            .execute(
                "UPDATE shared_mn_sessions SET abandon_txid = ?1, abandon_tx = NULL,
                     abandon_log = ?2, legacy_bound = 1, last_error = NULL
                 WHERE wallet_id = ?3 AND id = ?4",
                rusqlite::params![
                    txid.to_string(),
                    log.to_string(),
                    self.wallets[w].to_string(),
                    id
                ],
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    /// The latest abandon spend's log entry as an older build wrote it (no
    /// fee, no inputs, the legacy mark), its bytes kept: what a spend
    /// signed again with that entry's txid leaves.
    fn unlogged_fee(&self, w: usize, id: &str) {
        let row = self.row(w, id);
        let mut log: Vec<serde_json::Value> =
            serde_json::from_str(row.abandon_log.as_deref().unwrap()).unwrap();
        let entry = log.last_mut().unwrap();
        entry["fee"] = serde_json::Value::Null;
        entry["inputs"] = serde_json::json!([]);
        entry["legacy"] = serde_json::json!(true);
        let conn = rusqlite::Connection::open(self.session.data_dir().join(dw_appdb::APP_DB_FILE))
            .unwrap();
        let n = conn
            .execute(
                "UPDATE shared_mn_sessions SET abandon_log = ?1 WHERE wallet_id = ?2 AND id = ?3",
                rusqlite::params![
                    serde_json::Value::from(log).to_string(),
                    self.wallets[w].to_string(),
                    id
                ],
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    /// The tip at `height`, its time `time`.
    fn tip_at(&self, height: u32, time: u64) {
        self.tip(height);
        self.session.hub.tracker().set_tip_time(height, Some(time));
    }
}

fn reserved_coins(f: &Fx, w: usize, id: &str) -> Vec<OutPoint> {
    let mut coins = super::super::store::reserved_of(&f.row(w, id));
    coins.sort();
    coins
}

fn inputs(tx: &Transaction) -> Vec<OutPoint> {
    let mut v: Vec<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
    v.sort();
    v
}

/// A one-output spend of `coins` that no wallet signed: what history
/// records of another transaction.
fn foreign_spend(coins: &[OutPoint], tag: u64) -> Transaction {
    Transaction {
        version: 3,
        lock_time: 0,
        input: coins
            .iter()
            .map(|o| dashcore::TxIn {
                previous_output: *o,
                ..Default::default()
            })
            .collect(),
        output: vec![TxOut {
            value: tag,
            script_pubkey: dashcore::ScriptBuf::new(),
        }],
        special_transaction_payload: None,
    }
}

/// [`three_approved`] with B holding two coins it reserves both of, then
/// B's Signed Contribution out: B's two reserved coins.
fn two_coin_participant(f: &Fx) -> (String, Vec<OutPoint>) {
    f.credit(1, 40, 150 * COIN);
    f.credit(1, 41, 150 * COIN + 50_000);
    let id = three_approved_after_b(f, COIN, RewardChoice::Refund, 1);
    f.imported(1, &f.message(0, &id));
    f.sign(1, &id, 301 * COIN).unwrap();
    let coins = reserved_coins(f, 1, &id);
    assert_eq!(coins.len(), 2, "both coins reserved");
    (id, coins)
}

fn abandon_action(r: &SharedRecovery) -> Option<(u64, bool, Option<String>)> {
    r.actions.iter().find_map(|a| match a {
        RecoveryAction::Abandon {
            charge,
            spends,
            replaces,
        } => Some((*charge, *spends, replaces.clone())),
        _ => None,
    })
}

fn grant_exceeded<T: std::fmt::Debug>(r: Result<T, MasternodesError>) {
    assert_eq!(send_code(&r.unwrap_err()), "send.grant_exceeded");
}

/// T4 "registration before out": `discard` of what it reserves, nothing
/// else; the abandon calls refuse at its stage.
#[test]
fn test_recovery_before_out_is_a_discard() {
    let f = fx(3, true);
    let id = three_approved(&f);
    f.spv(true, 1);
    let r = f.recovery(1, &id);
    assert_eq!(r.out_at, None);
    assert_eq!(
        r.actions,
        [RecoveryAction::Discard {
            releases: 300 * COIN + 50_000
        }]
    );
    assert_eq!((r.settlement, r.blocked), (Settlement::None, None));
    assert_eq!(
        code(f.abandon(1, &id).unwrap_err()),
        "masternodes.shared_stage"
    );
    assert_eq!(
        code(f.resend_abandon(1, &id, COIN).unwrap_err()),
        "masternodes.shared_stage"
    );
    let all = f.run(f.mn().shared_sessions(f.wallets[1])).unwrap();
    assert_eq!(
        all[0].recovery,
        f.recovery(1, &id),
        "the list reads the same"
    );
}

/// T4 "coordinator `sent`, refused first hand-off": `resend` at the debit
/// and `abandon` at the one-input fee, each passing its call at that cap
/// and refused one duff under; with SPV stopped the resend is held back.
#[test]
fn test_a_refused_registration_lists_resend_and_abandon() {
    let mut f = fx(3, true);
    let id = three_approved(&f);
    f.both_signed(&id);
    f.net.reject_next();
    f.send(0, &id, 400 * COIN + FEE).unwrap();
    f.spv(true, 1);
    let debit = f.session_of(0, &id).debit.unwrap();
    let fee = spend_fee(1);
    let want = vec![
        RecoveryAction::Resend { charge: debit },
        RecoveryAction::Abandon {
            charge: fee,
            spends: true,
            replaces: None,
        },
    ];
    let r = f.recovery(0, &id);
    assert_eq!(r.actions, want);
    assert_stage(f.resend_abandon(0, &id, COIN).unwrap_err(), "sent");
    let e = r.handoff_error.unwrap();
    assert_eq!(
        (e.tx, e.outcome, e.code, e.resend_refused),
        (
            HandoffTx::Registration,
            HandoffErrorOutcome::Refused,
            f.row(0, &id).last_error,
            false
        )
    );
    assert!(r.out_at.is_some() && r.blocked.is_none());
    f.spv(false, 0);
    assert_eq!(f.recovery(0, &id).actions, want[1..]);
    f.restart();
    f.spv(true, 1);
    assert_eq!(f.recovery(0, &id).actions, want, "after a restart");
    grant_exceeded(f.send(0, &id, debit - 1));
    grant_exceeded(f.abandon_at(0, &id, fee - 1));
    let s = f.abandon_at(0, &id, fee).unwrap();
    assert_eq!(f.net.sent().len(), 1);
    assert_eq!(s.recovery.attributed, [f.net.sent()[0].txid().to_string()]);
}

/// T4 "coordinator `sent`, unknown hand-off": `in_flight` while its claim
/// holds the coin, no abandon; after a restart the abandon is listed.
#[test]
fn test_an_unknown_registration_hand_off_is_in_flight_until_restart() {
    let mut f = fx(3, true);
    let id = three_approved(&f);
    f.both_signed(&id);
    f.net.unconfirm_next();
    f.send(0, &id, 400 * COIN + FEE).unwrap();
    f.spv(true, 1);
    let r = f.recovery(0, &id);
    assert_eq!(
        r.handoff_error.as_ref().map(|e| e.outcome),
        Some(HandoffErrorOutcome::Unknown)
    );
    assert_eq!(abandon_action(&r), None);
    assert_eq!(
        r.blocked.as_ref().map(|b| b.reason),
        Some(BlockedReason::InFlight)
    );
    assert_stage(f.abandon(0, &id).unwrap_err(), "sent");
    f.restart();
    f.spv(true, 1);
    let r = f.recovery(0, &id);
    assert_eq!(abandon_action(&r), Some((spend_fee(1), true, None)));
    assert_eq!(r.blocked, None);
}

/// T4 "abandon A unknown" and "accepted, not recorded": A `uncertain`,
/// `resend_abandon` at A's fee, `abandon` refused `abandoning` without
/// taking its grant (D50); the replay hands the same bytes off; SPV stopped
/// holds it back.
#[test]
fn test_a_spend_back_that_may_be_out_is_only_sent_again() {
    for unknown in [false, true] {
        let mut f = fx(3, true);
        let id = abandoned_participant(&f);
        if unknown {
            f.net.unconfirm_next();
        }
        f.abandon(1, &id).unwrap();
        f.spv(true, 1);
        let row = f.row(1, &id);
        let a = row.abandon_txid.clone().unwrap();
        let fee = spend_fee(1);
        let r = f.recovery(1, &id);
        assert_eq!(r.settlement, Settlement::Pending { txid: a.clone() });
        let spend = r.abandon_spend.clone().unwrap();
        assert_eq!(
            (spend.txid.as_str(), spend.state, spend.legacy),
            (a.as_str(), Some(AbandonSpendState::Uncertain), false)
        );
        assert_eq!(r.attributed, std::slice::from_ref(&a));
        assert_eq!(r.actions, [RecoveryAction::ResendAbandon { charge: fee }]);
        assert_eq!(r.blocked, None);
        match unknown {
            false => assert_eq!(r.handoff_error, None, "sent: no reason"),
            true => {
                let e = r.handoff_error.unwrap();
                assert_eq!(
                    (e.tx, e.outcome, e.code),
                    (
                        HandoffTx::Abandon,
                        HandoffErrorOutcome::Unknown,
                        row.last_error
                    )
                );
            }
        }
        no_second_abandon(&f, &id);
        f.spv(false, 0);
        let r = f.recovery(1, &id);
        assert!(r.actions.is_empty());
        assert_eq!(
            r.blocked.as_ref().map(|b| b.reason),
            Some(BlockedReason::SpvStopped)
        );
        f.restart();
        f.spv(true, 1);
        assert_eq!(
            f.recovery(1, &id).actions,
            may_be_out(&f, &id, &a, fee),
            "after a restart"
        );
        no_second_abandon(&f, &id);
        let before = f.net.sent().len();
        grant_exceeded(f.resend_abandon(1, &id, fee - 1));
        f.resend_abandon(1, &id, fee).unwrap();
        let sent = f.net.sent();
        assert_eq!(sent.len(), before + 1);
        assert_eq!(sent.last().unwrap().txid().to_string(), a);
        let log = abandon_log(&f.row(1, &id));
        assert_eq!(log.len(), 1, "the same spend");
        assert_eq!(log[0].1.as_deref(), Some("sent"));
    }
}

/// D50, "no new spend while it may be out": a second abandon is refused
/// `abandoning` before its lease, so nothing is sent, logged or charged,
/// and its grant still redeems.
fn no_second_abandon(f: &Fx, id: &str) {
    let (sent, log) = (f.net.sent(), abandon_log(&f.row(1, id)));
    let grant = f.grant(1, COIN);
    let refused = f.run(
        f.mn()
            .abandon_shared_session(f.wallets[1], id.to_string(), grant.clone()),
    );
    assert_stage(refused.unwrap_err(), "abandoning");
    assert_eq!(f.net.sent(), sent, "nothing sent");
    assert_eq!(abandon_log(&f.row(1, id)), log, "nothing logged");
    f.session
        .vault()
        .redeem_grant(
            &grant,
            dw_vault::GrantKind::MasternodeOp,
            Some(&f.wallets[1].0),
        )
        .expect("the grant still redeems");
}

/// The actions of a spend back `a` that may be out: its replay, and once
/// the wallet's history holds it (its own send, kept across a restart)
/// Activity's Abandon too; its state is never `refused`.
fn may_be_out(f: &Fx, id: &str, a: &str, fee: u64) -> Vec<RecoveryAction> {
    let state = f.recovery(1, id).abandon_spend.and_then(|s| s.state);
    let mut want = vec![RecoveryAction::ResendAbandon { charge: fee }];
    match state {
        Some(AbandonSpendState::Uncertain) => {}
        Some(AbandonSpendState::InHistory) => {
            want.push(RecoveryAction::AbandonWalletTx { txid: a.into() })
        }
        other => panic!("{other:?}"),
    }
    want
}

/// T4 "Sol r4 seam" and "replay refused": A handed off and the engine gone
/// before its outcome: `uncertain` (`in_history` once the reloaded history
/// holds its own send), no reason, `resend_abandon`; a replay
/// the network refuses reads `maybe_sent`, never `not_sent`, across a
/// restart, and the abandon stays refused.
#[test]
fn test_a_spend_back_handed_off_before_a_crash_is_never_refused() {
    let mut f = fx(3, true);
    let id = abandoned_participant(&f);
    *f.seam.crash.lock().unwrap() = Some(CrashPoint::HandedOff);
    assert!(f.abandon(1, &id).is_err());
    *f.seam.crash.lock().unwrap() = None;
    f.restart();
    f.spv(true, 1);
    let a = f.net.sent()[0].txid().to_string();
    let r = f.recovery(1, &id);
    let want = may_be_out(&f, &id, &a, spend_fee(1));
    assert_eq!((r.attributed, r.handoff_error), (vec![a.clone()], None));
    assert_eq!(r.actions, want);
    assert_stage(f.abandon(1, &id).unwrap_err(), "abandoning");
    f.net.reject_next();
    f.resend_abandon(1, &id, COIN).unwrap();
    assert_eq!(f.net.sent().len(), 1, "refused");
    for restart in [false, true] {
        if restart {
            f.restart();
            f.spv(true, 1);
        }
        let log = abandon_log(&f.row(1, &id));
        assert_eq!(log.len(), 1, "the same spend");
        assert_eq!(
            (log[0].0.as_str(), log[0].1.as_deref()),
            (a.as_str(), Some("maybe_sent"))
        );
        let r = f.recovery(1, &id);
        assert_eq!(r.actions, may_be_out(&f, &id, &a, spend_fee(1)));
        assert_ne!(
            r.handoff_error.as_ref().map(|e| e.outcome),
            Some(HandoffErrorOutcome::Refused)
        );
        assert_stage(f.abandon(1, &id).unwrap_err(), "abandoning");
    }
}

/// T4 "abandon A refused at first dispatch, then B" with the same-coin
/// bound over two reserved coins: A `refused`, `abandon { replaces: A }` at
/// the two-input fee, `resend_shared_abandon` refused `abandon_refused`;
/// B takes every input A took; then A confirms after all.
#[test]
fn test_a_refused_spend_back_is_replaced_by_one_of_every_coin() {
    let f = fx(3, true);
    let (id, coins) = two_coin_participant(&f);
    f.spv(true, 1);
    f.net.reject_next();
    f.abandon(1, &id).unwrap();
    let row = f.row(1, &id);
    let a: Transaction =
        dashcore::consensus::deserialize(row.abandon_tx.as_deref().unwrap()).unwrap();
    assert_eq!(inputs(&a), coins, "A spends every reserved coin");
    let fee = spend_fee(2);
    assert_eq!(
        a.output[0].value,
        300 * COIN + 50_000 - fee,
        "the n-input fee"
    );
    let r = f.recovery(1, &id);
    assert_eq!(
        r.abandon_spend.as_ref().and_then(|s| s.state),
        Some(AbandonSpendState::Refused)
    );
    let e = r.handoff_error.unwrap();
    assert_eq!(
        (e.tx, e.outcome, e.code),
        (
            HandoffTx::Abandon,
            HandoffErrorOutcome::Refused,
            row.last_error
        )
    );
    assert_eq!(
        r.actions,
        [RecoveryAction::Abandon {
            charge: fee,
            spends: true,
            replaces: Some(a.txid().to_string())
        }]
    );
    assert_stage(
        f.resend_abandon(1, &id, COIN).unwrap_err(),
        "abandon_refused",
    );
    grant_exceeded(f.abandon_at(1, &id, fee - 1));
    let s = f.abandon_at(1, &id, fee).unwrap();
    let b = f.net.sent().pop().unwrap();
    assert_eq!(inputs(&b), inputs(&a), "B's inputs include A's");
    // A never left, so its change address may still be the next unused
    // one: B is then A signed again, its one entry `maybe_sent`; with a new
    // address B is a second entry.
    let want: Vec<String> = match b.txid() == a.txid() {
        true => vec![a.txid().to_string()],
        false => vec![a.txid().to_string(), b.txid().to_string()],
    };
    assert_eq!(s.recovery.attributed, want);
    f.mine(a.clone(), 60);
    f.tip(61);
    let r = f.recovery(1, &id);
    assert_eq!(
        r.settlement,
        Settlement::Confirming {
            by: SettledBy::AbandonTx,
            confirmations: 2,
            chainlocked: false
        }
    );
    assert!(r.actions.is_empty() && r.abandon_spend.as_ref().unwrap().state.is_none());
}

/// T4 "refused A, one of its inputs spent by another transaction X" (R1):
/// no spend back without every input A took; `blocked{X, coin_elsewhere}`.
#[test]
fn test_a_refused_spend_back_with_an_input_spent_elsewhere_is_blocked() {
    let f = fx(3, true);
    let (id, coins) = two_coin_participant(&f);
    f.spv(true, 1);
    f.net.reject_next();
    f.abandon(1, &id).unwrap();
    let x = foreign_spend(&coins[1..], 1);
    f.record(1, &x);
    assert_stage(f.abandon(1, &id).unwrap_err(), "funding_signed");
    assert!(f.net.sent().is_empty(), "no disjoint spend");
    let r = f.recovery(1, &id);
    assert_eq!(abandon_action(&r), None);
    let b = r.blocked.unwrap();
    assert_eq!(
        (b.txid, b.reason),
        (Some(x.txid().to_string()), BlockedReason::CoinElsewhere)
    );
}

/// T4 "abandon A accepted, recorded" and "A recorded, IS-locked": A
/// `in_history`, `resend_abandon` and Activity's `abandon_wallet_tx{A}`;
/// a lock blocks Activity's exit; SPV stopped blocks both.
#[test]
fn test_a_recorded_spend_back_lists_send_again_and_its_activity_exit() {
    let f = fx(3, true);
    let id = abandoned_participant(&f);
    f.abandon(1, &id).unwrap();
    let a = f.net.sent()[0].clone();
    f.record(1, &a);
    f.spv(true, 1);
    let txid = a.txid().to_string();
    let r = f.recovery(1, &id);
    assert_eq!(
        r.abandon_spend.as_ref().and_then(|s| s.state),
        Some(AbandonSpendState::InHistory)
    );
    assert_eq!(
        r.actions,
        [
            RecoveryAction::ResendAbandon {
                charge: spend_fee(1)
            },
            RecoveryAction::AbandonWalletTx { txid: txid.clone() }
        ]
    );
    assert_eq!(r.blocked, None);
    assert_stage(f.abandon(1, &id).unwrap_err(), "abandoning");
    grant_exceeded(f.resend_abandon(1, &id, spend_fee(1) - 1));
    f.resend_abandon(1, &id, spend_fee(1)).unwrap();
    assert_eq!(
        f.net.sent().last().unwrap().txid(),
        a.txid(),
        "the same bytes"
    );
    f.history_edit(1, a.txid(), |e| e.instant_locked = true);
    let r = f.recovery(1, &id);
    assert_eq!(
        r.actions,
        [RecoveryAction::ResendAbandon {
            charge: spend_fee(1)
        }]
    );
    let b = r.blocked.unwrap();
    assert_eq!(
        (b.txid, b.reason),
        (Some(txid), BlockedReason::InstantLocked)
    );
    f.history_edit(1, a.txid(), |e| e.instant_locked = false);
    f.spv(false, 0);
    let r = f.recovery(1, &id);
    assert!(r.actions.is_empty());
    assert_eq!(
        r.blocked.as_ref().map(|b| b.reason),
        Some(BlockedReason::SpvStopped)
    );
}

/// T4 "A abandoned in Activity, rescan running → done": `rescanning` with
/// no call accepted (`abandon_cancelled`), then `abandon { replaces: A }`
/// once the coin is back; `attributed` keeps A.
#[test]
fn test_a_spend_back_abandoned_in_activity_rescans_then_is_replaced() {
    let f = fx(3, true);
    let id = abandoned_participant(&f);
    f.abandon(1, &id).unwrap();
    let a = f.net.sent()[0].clone();
    let coin = a.input[0].previous_output;
    let (value, _) = f.coin_of(1, coin);
    f.record(1, &a);
    f.history_edit(1, a.txid(), |e| e.abandoned = true);
    f.spv(true, 1);
    let txid = a.txid().to_string();
    let r = f.recovery(1, &id);
    assert_eq!(
        r.abandon_spend.as_ref().and_then(|s| s.state),
        Some(AbandonSpendState::AbandonedInActivity)
    );
    assert!(r.actions.is_empty());
    let b = r.blocked.unwrap();
    assert_eq!(
        (b.txid, b.reason),
        (Some(txid.clone()), BlockedReason::Rescanning)
    );
    assert_stage(
        f.resend_abandon(1, &id, COIN).unwrap_err(),
        "abandon_cancelled",
    );
    assert_stage(f.abandon(1, &id).unwrap_err(), "funding_signed");
    f.credit_again(1, coin.txid.to_byte_array()[0], value);
    let r = f.recovery(1, &id);
    assert_eq!(
        r.actions,
        [RecoveryAction::Abandon {
            charge: spend_fee(1),
            spends: true,
            replaces: Some(txid.clone())
        }]
    );
    let s = f.abandon_at(1, &id, spend_fee(1)).unwrap();
    assert_eq!(f.net.sent().last().unwrap().input[0].previous_output, coin);
    assert_eq!(s.recovery.attributed.first(), Some(&txid));
}

/// Activity's Abandon of the spend back A stores its mark, then its sweep,
/// and flags A in the history only once the sweep is stored. A Send again
/// asked while that Abandon is held at its sweep is refused
/// `abandon_cancelled`; with the sweep then failed the history still holds
/// A live, and until the sweep is stored the read says A was abandoned in
/// Activity and lists only Activity's Abandon of A, no Send again and no
/// new spend, and both calls are refused. That Abandon again stores the
/// sweep, and once the coin is back a new spend (the user's, after it)
/// replaces A. Until then nothing goes out.
#[test]
fn test_a_spend_back_whose_activity_abandon_failed_its_sweep_is_never_sent_again() {
    let f = fx(3, true);
    let id = abandoned_participant(&f);
    f.abandon(1, &id).unwrap();
    let a = f.net.sent()[0].clone();
    let coin = a.input[0].previous_output;
    let (value, _) = f.coin_of(1, coin);
    f.record(1, &a);
    f.spv(true, 1);
    let replay = RecoveryAction::ResendAbandon {
        charge: spend_fee(1),
    };
    assert!(f.recovery(1, &id).actions.contains(&replay));
    let sent = f.net.sent().len();
    let (wallet, txid) = (f.wallets[1], a.txid());
    let activity_abandon = || {
        let s = Arc::clone(&f.session);
        f.session.rt.spawn(async move {
            let manager = s.manager()?;
            s.abandon_inner(&manager, wallet, txid).await
        })
    };

    let pause = Arc::new(crate::coins::TestPause::default());
    *f.session.hub.pause_abandon_sweep.lock().unwrap() = Some(Arc::clone(&pause));
    let abandoning = activity_abandon();
    assert!(pause.wait_reached(std::time::Duration::from_secs(60)));
    let resending = {
        let (mn, grant, id) = (f.mn(), f.grant(1, COIN), id.clone());
        f.session
            .rt
            .spawn(async move { mn.resend_shared_abandon(wallet, id, grant).await })
    };
    (f.session.hub.fail_abandon_sweep).store(true, std::sync::atomic::Ordering::SeqCst);
    pause.release.notify_one();
    let swept = f.run(abandoning).unwrap();
    assert!(
        matches!(&swept, Err(crate::EngineError::Storage(_))),
        "{swept:?}"
    );
    assert_stage(f.run(resending).unwrap().unwrap_err(), "abandon_cancelled");
    f.history_edit(1, txid, |e| assert!(!e.abandoned, "flagged"));

    let finish = RecoveryAction::AbandonWalletTx {
        txid: txid.to_string(),
    };
    let r = f.recovery(1, &id);
    assert_eq!(
        r.abandon_spend.as_ref().and_then(|s| s.state),
        Some(AbandonSpendState::AbandonedInActivity)
    );
    assert_eq!(r.actions, [finish], "Activity's Abandon finishes the sweep");
    assert_eq!(r.blocked, None);
    assert_stage(
        f.resend_abandon(1, &id, COIN).unwrap_err(),
        "abandon_cancelled",
    );
    assert_stage(f.abandon(1, &id).unwrap_err(), "abandoning");
    assert_eq!(f.net.sent().len(), sent, "nothing went out");

    f.run(activity_abandon()).unwrap().unwrap();
    f.history_edit(1, txid, |e| assert!(e.abandoned, "not flagged"));
    f.credit_again(1, coin.txid.to_byte_array()[0], value);
    let r = f.recovery(1, &id);
    assert_eq!(
        r.actions,
        [RecoveryAction::Abandon {
            charge: spend_fee(1),
            spends: true,
            replaces: Some(txid.to_string())
        }]
    );
    f.abandon_at(1, &id, spend_fee(1)).unwrap();
    assert_eq!(f.net.sent().len(), sent + 1, "the new spend went out");
    assert_eq!(f.net.sent()[sent].input[0].previous_output, coin);
}

/// A spend back's Abandon mark that can't be read counts as stored: the
/// read says A was abandoned in Activity and lists Activity's Abandon of
/// it, no Send again, and the call is refused `abandon_cancelled`; with the read back, Send again is
/// listed again.
#[test]
fn test_a_spend_back_whose_abandon_mark_cannot_be_read_is_not_sent_again() {
    let f = fx(3, true);
    let id = abandoned_participant(&f);
    f.abandon(1, &id).unwrap();
    let a = f.net.sent()[0].clone();
    f.record(1, &a);
    f.spv(true, 1);
    let replay = RecoveryAction::ResendAbandon {
        charge: spend_fee(1),
    };
    let path = f.session.data_dir().join(dw_appdb::APP_DB_FILE);
    let unreadable = || {
        dw_appdb::fault::arm(
            &path,
            &format!("settings.read_abandoned:{}", a.txid()),
            dw_appdb::fault::Stage::Before,
        )
    };
    let sent = f.net.sent().len();

    unreadable();
    let r = f.recovery(1, &id);
    assert_eq!(
        r.abandon_spend.as_ref().and_then(|s| s.state),
        Some(AbandonSpendState::AbandonedInActivity)
    );
    assert_eq!(
        r.actions,
        [RecoveryAction::AbandonWalletTx {
            txid: a.txid().to_string()
        }]
    );
    unreadable();
    assert_stage(
        f.resend_abandon(1, &id, COIN).unwrap_err(),
        "abandon_cancelled",
    );
    dw_appdb::fault::disarm(&path);
    assert_eq!(f.net.sent().len(), sent, "A never went out again");
    assert!(f.recovery(1, &id).actions.contains(&replay));
}

/// A spend back not recorded (`uncertain`, its coins still held) whose
/// Abandon mark can't be read: no Send again, and no new spend either,
/// which `abandon_shared_session` would refuse while A may be out.
#[test]
fn test_an_uncertain_spend_back_with_an_unreadable_mark_lists_no_new_spend() {
    let f = fx(3, true);
    let id = abandoned_participant(&f);
    f.abandon(1, &id).unwrap();
    let a = f.net.sent()[0].clone();
    f.spv(true, 1);
    let r = f.recovery(1, &id);
    assert_eq!(
        r.abandon_spend.as_ref().and_then(|s| s.state),
        Some(AbandonSpendState::Uncertain)
    );
    assert_eq!(
        r.actions,
        [RecoveryAction::ResendAbandon {
            charge: spend_fee(1)
        }]
    );
    let path = f.session.data_dir().join(dw_appdb::APP_DB_FILE);
    dw_appdb::fault::arm(
        &path,
        &format!("settings.read_abandoned:{}", a.txid()),
        dw_appdb::fault::Stage::Before,
    );
    let r = f.recovery(1, &id);
    dw_appdb::fault::disarm(&path);
    assert!(r.actions.is_empty(), "{r:?}");
    assert_stage(f.abandon(1, &id).unwrap_err(), "abandoning");
}

/// T4 legacy row, whole reservation unspent, before and after
/// `abandon_spend.overdue`: nothing listed before (the call would accept a
/// bounded spend), then `abandon { replaces: A }` taking every reserved
/// coin, which clears the bound; `resend_shared_abandon` is `legacy`.
#[test]
fn test_a_legacy_spend_back_is_replaced_once_overdue_by_the_whole_reservation() {
    let mut f = fx(3, true);
    let (id, coins) = two_coin_participant(&f);
    let a = foreign_spend(&coins[..1], 1);
    f.legacy(1, &id, a.txid(), 1_000);
    f.restart();
    f.spv(true, 1);
    f.tip_at(100, 1_000 + OVERDUE_SECS - 1);
    let r = f.recovery(1, &id);
    let spend = r.abandon_spend.clone().unwrap();
    assert_eq!(
        (spend.legacy, spend.overdue, spend.state),
        (true, false, Some(AbandonSpendState::Legacy))
    );
    assert!(r.actions.is_empty() && r.blocked.is_none(), "{r:?}");
    assert_eq!(r.handoff_error, None, "no reason for a legacy spend");
    assert_stage(f.resend_abandon(1, &id, COIN).unwrap_err(), "legacy");
    f.tip_at(100, 1_000 + OVERDUE_SECS);
    let r = f.recovery(1, &id);
    assert!(r.abandon_spend.as_ref().unwrap().overdue);
    let fee = spend_fee(2);
    assert_eq!(
        abandon_action(&r),
        Some((fee, true, Some(a.txid().to_string())))
    );
    f.abandon_at(1, &id, fee).unwrap();
    assert_eq!(inputs(&f.net.sent()[0]), coins, "the whole reservation");
    assert!(!f.row(1, &id).legacy_bound, "the bound cleared");
}

/// T4 "legacy, two-input reservation, one input held by an unconfirmed
/// spender X" (R1): nothing before overdue, `blocked{X, coin_elsewhere}`
/// after, the abandon refused throughout; once X is abandoned and its coin
/// back, the replacement takes both.
#[test]
fn test_a_legacy_spend_back_with_a_coin_elsewhere_waits_for_it() {
    let mut f = fx(3, true);
    let (id, coins) = two_coin_participant(&f);
    let a = foreign_spend(&coins[..1], 1);
    f.legacy(1, &id, a.txid(), 1_000);
    f.restart();
    f.spv(true, 1);
    let (value, _) = f.coin_of(1, coins[1]);
    let x = foreign_spend(&coins[1..], 2);
    f.record(1, &x);
    f.tip_at(100, 1_000);
    let r = f.recovery(1, &id);
    assert!(r.actions.is_empty() && r.blocked.is_none(), "{r:?}");
    assert_stage(f.abandon(1, &id).unwrap_err(), "funding_signed");
    f.tip_at(100, 1_000 + OVERDUE_SECS);
    let r = f.recovery(1, &id);
    assert_eq!(abandon_action(&r), None);
    let b = r.blocked.unwrap();
    assert_eq!(
        (b.txid, b.reason),
        (Some(x.txid().to_string()), BlockedReason::CoinElsewhere)
    );
    assert_stage(f.abandon(1, &id).unwrap_err(), "funding_signed");
    assert!(f.net.sent().is_empty(), "no disjoint spend");
    f.history_edit(1, x.txid(), |e| e.abandoned = true);
    f.credit_again(1, 41, value);
    let r = f.recovery(1, &id);
    assert_eq!(
        abandon_action(&r),
        Some((spend_fee(2), true, Some(a.txid().to_string())))
    );
    f.abandon_at(1, &id, spend_fee(2)).unwrap();
    assert_eq!(inputs(&f.net.sent()[0]), coins);
}

/// T4 "legacy one-input A in history, two reserved coins, the other free"
/// (R3) and "... no peers": Activity's exits on A whatever the coin test
/// says; with no peers its Resend is left out. Then "legacy one-input A
/// abandoned in Activity, rescan running": `rescanning`, not
/// `coin_elsewhere`; once the coin is back, the replacement takes both.
#[test]
fn test_a_recorded_legacy_spend_back_keeps_its_activity_exits() {
    let mut f = fx(3, true);
    let (id, coins) = two_coin_participant(&f);
    let a = foreign_spend(&coins[..1], 1);
    f.legacy(1, &id, a.txid(), 1_000);
    f.restart();
    let (value, _) = f.coin_of(1, coins[0]);
    f.record(1, &a);
    f.spv(true, 1);
    let txid = a.txid().to_string();
    let r = f.recovery(1, &id);
    assert_eq!(
        r.abandon_spend.as_ref().and_then(|s| s.state),
        Some(AbandonSpendState::InHistory)
    );
    assert_eq!(
        r.actions,
        [
            RecoveryAction::AbandonWalletTx { txid: txid.clone() },
            RecoveryAction::ResendWalletTx { txid: txid.clone() }
        ]
    );
    assert_stage(f.abandon(1, &id).unwrap_err(), "abandoning");
    assert_stage(f.resend_abandon(1, &id, COIN).unwrap_err(), "legacy");
    f.spv(true, 0);
    assert_eq!(
        f.recovery(1, &id).actions,
        [RecoveryAction::AbandonWalletTx { txid: txid.clone() }],
        "no peers: no Resend"
    );
    f.spv(true, 1);
    f.history_edit(1, a.txid(), |e| e.abandoned = true);
    let r = f.recovery(1, &id);
    assert!(r.actions.is_empty());
    let b = r.blocked.unwrap();
    assert_eq!(
        (b.txid, b.reason),
        (Some(txid.clone()), BlockedReason::Rescanning)
    );
    assert_stage(f.abandon(1, &id).unwrap_err(), "funding_signed");
    f.credit_again(1, 40, value);
    let r = f.recovery(1, &id);
    assert_eq!(abandon_action(&r), Some((spend_fee(2), true, Some(txid))));
    f.abandon_at(1, &id, spend_fee(2)).unwrap();
    assert_eq!(inputs(&f.net.sent()[0]), coins);
    assert!(!f.row(1, &id).legacy_bound);
}

/// T4 "legacy two-abandon upgrade": an older build's P spent
/// x and stays unconfirmed, its A spent y and overwrote P's id. With A
/// abandoned in Activity, before and after its coin is back: `blocked{P,
/// coin_elsewhere}` and no spend of y alone; once P is abandoned too and x
/// back, the replacement takes both and `attributed` is `[A, B]`.
#[test]
fn test_a_two_abandon_upgrade_never_spends_one_coin_alone() {
    let mut f = fx(3, true);
    let (id, coins) = two_coin_participant(&f);
    let (x, y) = (coins[0], coins[1]);
    let p = foreign_spend(&[x], 1);
    let a = foreign_spend(&[y], 2);
    f.legacy(1, &id, a.txid(), 1_000);
    f.restart();
    f.spv(true, 1);
    let ((xv, _), (yv, _)) = (f.coin_of(1, x), f.coin_of(1, y));
    f.record(1, &p);
    f.record(1, &a);
    f.history_edit(1, a.txid(), |e| e.abandoned = true);
    f.tip_at(100, 1_000 + OVERDUE_SECS);
    let elsewhere = |f: &Fx| {
        let r = f.recovery(1, &id);
        assert_eq!(abandon_action(&r), None, "{r:?}");
        let b = r.blocked.unwrap();
        assert_eq!(
            (b.txid, b.reason),
            (Some(p.txid().to_string()), BlockedReason::CoinElsewhere)
        );
        assert_eq!(r.attributed, [a.txid().to_string()]);
        assert_stage(f.abandon(1, &id).unwrap_err(), "funding_signed");
    };
    elsewhere(&f);
    f.credit_again(1, y.txid.to_byte_array()[0], yv);
    elsewhere(&f);
    assert!(f.net.sent().is_empty());
    f.history_edit(1, p.txid(), |e| e.abandoned = true);
    f.credit_again(1, x.txid.to_byte_array()[0], xv);
    let r = f.recovery(1, &id);
    assert_eq!(
        abandon_action(&r),
        Some((spend_fee(2), true, Some(a.txid().to_string())))
    );
    let s = f.abandon_at(1, &id, spend_fee(2)).unwrap();
    let b = f.net.sent()[0].clone();
    assert_eq!(inputs(&b), coins);
    assert_eq!(
        s.recovery.attributed,
        [a.txid().to_string(), b.txid().to_string()]
    );
    assert!(!f.row(1, &id).legacy_bound);
}

/// T4 settlement: A in a block is `confirming{abandon_tx}` with its
/// depth, never `uncertain`; buried it is `settled`, `released` once the
/// follower clears the locks, and stays so across a restart; every call
/// refuses `conflicted`. A reorg takes it back to `pending`.
#[test]
fn test_a_spend_back_in_a_block_settles_and_releases() {
    let mut f = fx(3, true);
    let id = abandoned_participant(&f);
    f.abandon(1, &id).unwrap();
    f.spv(true, 1);
    let a = f.net.sent()[0].clone();
    f.mine(a.clone(), 60);
    for (tip, n) in [(60, 1), (64, 5)] {
        f.tip(tip);
        let r = f.recovery(1, &id);
        assert_eq!(
            r.settlement,
            Settlement::Confirming {
                by: SettledBy::AbandonTx,
                confirmations: n,
                chainlocked: false
            }
        );
        assert!(r.actions.is_empty() && r.blocked.is_none());
        assert_eq!(r.abandon_spend.as_ref().unwrap().state, None);
    }
    assert_stage(f.abandon(1, &id).unwrap_err(), "conflicted");
    assert_stage(f.resend_abandon(1, &id, COIN).unwrap_err(), "conflicted");
    f.seam.shared_mined.lock().unwrap().clear();
    assert_eq!(
        f.recovery(1, &id).settlement,
        Settlement::Pending {
            txid: a.txid().to_string()
        },
        "a reorg"
    );
    f.mine(a.clone(), 60);
    f.tip(65);
    assert_eq!(
        f.recovery(1, &id).settlement,
        Settlement::Settled {
            by: SettledBy::AbandonTx,
            released: false
        }
    );
    f.follow();
    let settled = Settlement::Settled {
        by: SettledBy::AbandonTx,
        released: true,
    };
    assert_eq!(f.recovery(1, &id).settlement, settled);
    let mined = f.seam.shared_mined.lock().unwrap().clone();
    f.restart();
    *f.seam.shared_mined.lock().unwrap() = mined;
    f.tip(65);
    assert_eq!(f.recovery(1, &id).settlement, settled, "after a restart");
}

/// T4 "ChainLock at 1 confirmation" and "another conflicting spend wins",
/// with A absent from history (R4): `settled` at once under a ChainLock;
/// another spend reads `other_spend`; the calls refuse `conflicted`.
#[test]
fn test_another_spend_in_a_block_settles_as_other_spend() {
    let f = fx(3, true);
    let id = abandoned_participant(&f);
    f.abandon(1, &id).unwrap();
    let coin = f.net.sent()[0].input[0].previous_output;
    let x = foreign_spend(&[coin], 7);
    f.mine(x, 60);
    f.tip(60);
    let r = f.recovery(1, &id);
    assert_eq!(
        r.settlement,
        Settlement::Confirming {
            by: SettledBy::OtherSpend,
            confirmations: 1,
            chainlocked: false
        }
    );
    assert!(r.actions.is_empty());
    assert_stage(f.resend_abandon(1, &id, COIN).unwrap_err(), "conflicted");
    f.session.hub.tracker().set_chainlock_height(60);
    assert_eq!(
        f.recovery(1, &id).settlement,
        Settlement::Settled {
            by: SettledBy::OtherSpend,
            released: false
        }
    );
}

/// T4 "registration wins against the abandon" with A absent from history:
/// stage `confirmed`, no recovery action, no spend-back state; both calls
/// refuse `confirmed`.
#[test]
fn test_a_registration_that_wins_lists_nothing() {
    let f = fx(3, true);
    let id = three_approved(&f);
    f.both_signed(&id);
    f.net.reject_next();
    f.send(0, &id, 400 * COIN + FEE).unwrap();
    f.abandon(0, &id).unwrap();
    f.spv(true, 1);
    let registration: Transaction =
        dashcore::consensus::deserialize(&f.row(0, &id).signed_tx.unwrap()).unwrap();
    f.mine(registration, 60);
    f.tip(60);
    let s = f.session_of(0, &id);
    assert!(
        matches!(s.stage, SharedStage::Confirmed { .. }),
        "{:?}",
        s.stage
    );
    assert!(s.recovery.actions.is_empty() && s.recovery.blocked.is_none());
    assert_eq!(s.recovery.settlement, Settlement::None);
    assert_eq!(s.recovery.abandon_spend.unwrap().state, None);
    assert_stage(f.abandon(0, &id).unwrap_err(), "confirmed");
    assert_stage(f.resend_abandon(0, &id, COIN).unwrap_err(), "confirmed");
}

/// #438: on mainnet `send_shared_registration` refuses, so a coordinator's
/// `sent` session whose hand-off was refused lists no `resend` (the read on
/// regtest lists it). The row is the regtest session's, its envelope named
/// mainnet and read back at `draft`, whose addresses and service mainnet
/// doesn't check; out, the row reads `sent` whatever the envelope's stage.
#[test]
fn test_a_mainnet_session_lists_no_resend() {
    let f = fx(3, true);
    let id = three_approved(&f);
    f.both_signed(&id);
    f.net.reject_next();
    f.send(0, &id, 400 * COIN + FEE).unwrap();
    f.spv(true, 1);
    let debit = f.session_of(0, &id).debit.unwrap();
    let resend = RecoveryAction::Resend { charge: debit };
    assert!(f.recovery(0, &id).actions.contains(&resend));
    let mut row = f.row(0, &id);
    let mut envelope = parse(&row.envelope);
    envelope.network = dashcore::Network::Mainnet;
    envelope.stage = Stage::Draft;
    row.envelope = envelope.to_text();

    let (out_at, signed, txid, error) = (
        row.out_at.take().unwrap(),
        row.signed_tx.take().unwrap(),
        row.txid.take().unwrap(),
        row.last_error.take(),
    );

    let m = fx_on(DashNetwork::Mainnet, 1, true);
    let (db, w) = (m.db(), m.wallets[0].to_string());
    db.insert_shared_session_durable(&w, &row, 1).unwrap();
    assert!(
        db.mark_shared_session_out_durable(&w, &id, &signed, &txid, out_at)
            .unwrap()
    );
    db.record_shared_session_attempt(&w, &id, error.as_deref())
        .unwrap();
    m.spv(true, 1);
    let s = m.session_of(0, &id);
    assert_eq!(s.stage, SharedStage::Sent);
    assert!(!s.recovery.actions.contains(&resend), "{:?}", s.recovery);
    let e = m.send(0, &id, 400 * COIN + FEE).unwrap_err();
    assert!(
        matches!(&e, MasternodesError::Unsupported { feature, .. } if feature == "protx_sending"),
        "{e:?}"
    );
}

/// The resend and abandon calls with the switch off are `unsupported`, and
/// the read lists no call that hands bytes off.
#[test]
fn test_sending_off_lists_no_hand_off() {
    let f = fx(3, true);
    let id = abandoned_participant(&f);
    f.net.reject_next();
    f.abandon(1, &id).unwrap();
    f.spv(true, 1);
    assert!(abandon_action(&f.recovery(1, &id)).is_some());
    let off = f.sending_off();
    let s = f.run(off.shared_session(f.wallets[1], id.clone())).unwrap();
    assert!(s.recovery.actions.is_empty(), "{:?}", s.recovery);
    let e = f
        .run(off.resend_shared_abandon(f.wallets[1], id.clone(), f.grant(1, COIN)))
        .unwrap_err();
    assert!(matches!(e, MasternodesError::Unsupported { .. }), "{e:?}");
}

/// `resend_shared_abandon`'s inputs check (MG-04 D7): while another
/// pending spend holds an input of the saved spend, the call refuses and
/// the read leaves the replay out.
#[test]
fn test_a_spend_back_whose_input_another_spend_holds_is_not_sent_again() {
    let f = fx(3, true);
    let id = abandoned_participant(&f);
    f.abandon(1, &id).unwrap();
    f.spv(true, 1);
    let coin = f.net.sent()[0].input[0].previous_output;
    let other = crate::send::SpendOwner::Tx(Txid::from_byte_array([9; 32]));
    f.session.spends.reserve(f.wallets[1], other, [coin]);
    let r = f.recovery(1, &id);
    assert!(r.actions.is_empty(), "{r:?}");
    let e = f.resend_abandon(1, &id, COIN).unwrap_err();
    assert_eq!(send_code(&e), "send.outpoint_unavailable");
    assert_eq!(f.net.sent().len(), 1);
    f.session.spends.release(&f.wallets[1], other);
    assert_eq!(
        f.recovery(1, &id).actions,
        [RecoveryAction::ResendAbandon {
            charge: spend_fee(1)
        }]
    );
}

/// A spend back whose log entry has no fee (an older build's entry, kept
/// by a spend signed again with its txid) is never replayed uncharged: its
/// replay is listed and charged what its bytes pay, its coin valued while
/// held, then from its history entry; with neither, no replay is listed
/// and the call refuses `legacy`, Activity's abandon its exit.
#[test]
fn test_a_spend_back_logged_without_its_fee_is_charged_what_it_pays() {
    let f = fx(3, true);
    let id = abandoned_participant(&f);
    f.abandon(1, &id).unwrap();
    f.spv(true, 1);
    let (fee, spend) = (spend_fee(1), f.net.sent()[0].clone());
    let replay = RecoveryAction::ResendAbandon { charge: fee };
    f.unlogged_fee(1, &id);
    assert!(f.row(1, &id).abandon_tx.is_some());
    let r = f.recovery(1, &id);
    assert_eq!(
        r.abandon_spend.as_ref().map(|s| (s.legacy, s.state)),
        Some((true, Some(AbandonSpendState::Uncertain)))
    );
    assert_eq!(r.actions, std::slice::from_ref(&replay));
    grant_exceeded(f.resend_abandon(1, &id, fee - 1));
    assert_eq!(f.net.sent().len(), 1);
    f.resend_abandon(1, &id, fee).unwrap();
    assert_eq!(f.net.sent().len(), 2);

    f.record(1, &spend);
    let r = f.recovery(1, &id);
    assert_eq!(
        r.abandon_spend.as_ref().and_then(|s| s.state),
        Some(AbandonSpendState::InHistory)
    );
    assert!(r.actions.contains(&replay), "{r:?}");
    grant_exceeded(f.resend_abandon(1, &id, fee - 1));

    f.history_edit(1, spend.txid(), |e| e.own_inputs.clear());
    let r = f.recovery(1, &id);
    assert_eq!(
        r.actions,
        [RecoveryAction::AbandonWalletTx {
            txid: spend.txid().to_string()
        }]
    );
    for cap in [0, COIN] {
        assert_stage(f.resend_abandon(1, &id, cap).unwrap_err(), "legacy");
    }
    assert_eq!(f.net.sent().len(), 2, "nothing sent again");
}

/// Wallet `w`'s row of `id` written back as `before` left it (a database
/// restored from a backup older than the signing).
fn restore(f: &Fx, w: usize, id: &str, before: &dw_appdb::SharedSessionRow) {
    let conn =
        rusqlite::Connection::open(f.session.data_dir().join(dw_appdb::APP_DB_FILE)).unwrap();
    let n = conn
        .execute(
            "UPDATE shared_mn_sessions SET envelope = ?1, contribution = ?2,
                 approved_terms = ?3, signed_tx = ?4, txid = ?5, out_at = ?6,
                 last_error = ?7, updated_at = ?8, reserved = ?9
             WHERE wallet_id = ?10 AND id = ?11",
            rusqlite::params![
                before.envelope,
                before.contribution,
                before.approved_terms,
                before.signed_tx,
                before.txid,
                before.out_at.map(|v| v as i64),
                before.last_error,
                before.updated_at as i64,
                before.reserved,
                f.wallets[w].to_string(),
                id
            ],
        )
        .unwrap();
    assert_eq!(n, 1);
}

/// MG-10 EP 4 (funds review): a copy whose import is refused but which
/// carries this wallet's signature over its own input (the wallet signed
/// on another install, or its database was restored from before the
/// signing) may still reach the network: the session reads out, of
/// unknown outcome, with the abandon as its exit and no discard.
#[test]
fn test_a_refused_copy_carrying_our_signature_reads_possibly_out() {
    let f = fx(3, true);
    let id = three_approved(&f);
    f.imported(1, &f.message(0, &id));
    let before = f.row(1, &id);
    let signed = f.sign(1, &id, 301 * COIN).unwrap();
    restore(&f, 1, &id, &before);
    f.spv(true, 1);
    assert!(f.row(1, &id).out_at.is_none());
    assert!(
        (f.recovery(1, &id).actions.iter()).any(|a| matches!(a, RecoveryAction::Discard { .. })),
        "not out: discard"
    );
    let mut copy = parse(&signed.message.text);
    copy.revision += 1;
    assert_message(f.import(1, &copy.to_text()).unwrap_err(), "conflict");
    let row = f.row(1, &id);
    assert!(row.out_at.is_some(), "{row:?}");
    assert_eq!(row.last_error.as_deref(), Some("send.broadcast_unknown"));
    let s = f.session_of(1, &id);
    assert_eq!(s.stage, SharedStage::FundingSigned);
    let r = s.recovery;
    assert!(
        !r.actions
            .iter()
            .any(|a| matches!(a, RecoveryAction::Discard { .. })),
        "{r:?}"
    );
    assert_eq!(abandon_action(&r), Some((spend_fee(1), true, None)));
    assert_eq!(
        r.handoff_error.map(|e| e.outcome),
        Some(HandoffErrorOutcome::Unknown)
    );
    let e = f
        .run(f.mn().discard_shared_session(f.wallets[1], id.clone()))
        .unwrap_err();
    assert_eq!(code(e), "masternodes.shared_stage");
}

/// `copy` one revision on, each signed input's scriptSig rewritten by `edit`
/// from its signature (hash type last) and key pushes, with the input's
/// index and the transaction.
fn tampered(copy: &Session, edit: impl Fn(&SpecialTx, usize, &[u8], &[u8]) -> ScriptBuf) -> String {
    let mut copy = copy.clone();
    copy.revision += 1;
    let mut tx = decode_frozen(&copy).unwrap().tx;
    let signed: Vec<usize> = (0..tx.input.len())
        .filter(|&i| !tx.input[i].script_sig.is_empty())
        .collect();
    assert!(!signed.is_empty());
    for i in signed {
        let mut pushes = tx.input[i].script_sig.instructions();
        let (Some(Ok(Instruction::PushBytes(sig))), Some(Ok(Instruction::PushBytes(key)))) =
            (pushes.next(), pushes.next())
        else {
            panic!("not a P2PKH scriptSig");
        };
        let (sig, key) = (sig.as_bytes().to_vec(), key.as_bytes().to_vec());
        tx.input[i].script_sig = edit(&tx, i, &sig, &key);
    }
    copy.protx = hex::encode(tx.to_bytes());
    copy.to_text()
}

fn p2pkh_sig(sig: &[u8], key: &[u8]) -> ScriptBuf {
    Builder::new()
        .push_slice(<&PushBytes>::try_from(sig).unwrap())
        .push_slice(<&PushBytes>::try_from(key).unwrap())
        .into_script()
}

/// MG-10 EP 4's negative control: a refused copy whose scriptSigs hold no
/// valid `SIGHASH_ALL` signature by this wallet over its own inputs (a
/// broken signature, another hash type, none, or a valid one by a key the
/// wallet doesn't hold) leaves the session not out, Discard listed. The
/// genuine copy, refused the same way, then reads out.
#[test]
fn test_a_refused_copy_without_our_valid_signature_stays_discardable() {
    let f = fx(3, true);
    let id = three_approved(&f);
    f.imported(1, &f.message(0, &id));
    let before = f.row(1, &id);
    let signed = f.sign(1, &id, 301 * COIN).unwrap();
    restore(&f, 1, &id, &before);
    f.spv(true, 1);
    let copy = parse(&signed.message.text);
    let stage = f.session_of(1, &id).stage;
    let foreign = SecretKey::from_secret_bytes([0x11; 32]).unwrap();
    let foreign_key = PublicKey::new(foreign.public_key());
    let copies = [
        (
            "broken signature",
            tampered(&copy, |_, _, sig, key| {
                let mut sig = sig.to_vec();
                let last_s = sig.len() - 2;
                sig[last_s] ^= 1;
                p2pkh_sig(&sig, key)
            }),
        ),
        (
            "another hash type",
            tampered(&copy, |_, _, sig, key| {
                let mut sig = sig.to_vec();
                *sig.last_mut().unwrap() = 0x81;
                p2pkh_sig(&sig, key)
            }),
        ),
        (
            "no signature",
            tampered(&copy, |_, _, _, _| ScriptBuf::new()),
        ),
        (
            "a key the wallet doesn't hold",
            tampered(&copy, |tx, i, _, _| {
                let code = ScriptBuf::new_p2pkh(&foreign_key.pubkey_hash());
                let hash = tx.signature_hash_all(i, &code).unwrap();
                let mut sig = ecdsa::sign(Message::from_digest(hash), &foreign)
                    .serialize_der()
                    .to_vec();
                sig.push(0x01);
                p2pkh_sig(&sig, &foreign_key.to_bytes())
            }),
        ),
    ];
    for (case, text) in copies {
        assert_message(f.import(1, &text).unwrap_err(), "conflict");
        let row = f.row(1, &id);
        assert!(row.out_at.is_none(), "{case}: {row:?}");
        assert_eq!(row.last_error, before.last_error, "{case}");
        assert_eq!(f.session_of(1, &id).stage, stage, "{case}");
        assert!(
            (f.recovery(1, &id).actions.iter())
                .any(|a| matches!(a, RecoveryAction::Discard { .. })),
            "{case}"
        );
    }
    let mut genuine = copy;
    genuine.revision += 1;
    assert_message(f.import(1, &genuine.to_text()).unwrap_err(), "conflict");
    assert!(f.row(1, &id).out_at.is_some());
}

/// A coordinator whose database was restored from a backup of its draft
/// taken before a fee change re-selected its coins: the draft records
/// `{c1, c2}`, the registration sent spends `{c1, c3}`. Its Final Record,
/// refused `older`, marks the session possibly out with the registration's
/// bytes. The registration, its coins c1 and c3, and the held c2.
fn restored_coordinator(f: &Fx) -> (String, Transaction, OutPoint) {
    f.credit(1, 2, 300 * COIN + 50_000);
    f.credit(2, 3, 350 * COIN);
    let c1 = f.credit(0, 1, 399 * COIN + COIN / 2);
    let c2 = f.credit(0, 4, COIN);
    let req = request(&[("A", 400), ("B", 300), ("C", 300)], FEE);
    let id = start(f, 0, req).unwrap().session.id;
    let mut drafted = vec![c1, c2];
    drafted.sort();
    assert_eq!(reserved_coins(f, 0, &id), drafted);
    let before = f.row(0, &id);
    let c3 = f.credit(0, 5, 2 * COIN);
    f.run(f.mn().set_shared_fee(f.wallets[0], id.clone(), 2 * FEE))
        .unwrap();
    let mut chosen = vec![c1, c3];
    chosen.sort();
    assert_eq!(reserved_coins(f, 0, &id), chosen, "re-selected");
    let invitation = f.message(0, &id);
    for w in [1, 2] {
        f.imported(w, &invitation);
    }
    for w in [1, 2] {
        let joined = f.join(w, &id);
        f.imported(0, &joined.message.text);
    }
    let locked = f.lock(0, &id).unwrap();
    for w in [1, 2] {
        f.imported(w, &locked.message.text);
        let approval = f.approve(w, &id).unwrap();
        f.imported(0, &approval.message.text);
    }
    f.both_signed(&id);
    f.send(0, &id, 401 * COIN).unwrap();
    let registration = f.net.sent()[0].clone();
    assert!(chosen.iter().all(|c| inputs(&registration).contains(c)));
    assert!(!inputs(&registration).contains(&c2));
    let record = f.message(0, &id);
    restore(f, 0, &id, &before);
    assert_message(f.import(0, &record).unwrap_err(), "older");
    let row = f.row(0, &id);
    assert!(row.out_at.is_some(), "{row:?}");
    assert_eq!(row.txid, Some(registration.txid().to_string()));
    assert_eq!(reserved_coins(f, 0, &id), drafted);
    f.record(0, &registration);
    assert!(f.held_coin(0, c1).is_none() && f.held_coin(0, c2).is_some());
    f.spv(true, 1);
    (id, registration, c2)
}

/// [`restored_coordinator`]: the held c2 alone isn't an input of the
/// registration, so its spend back wouldn't conflict with it; the read and
/// the call take Activity's abandon of the registration as the exit instead.
#[test]
fn test_a_restored_draft_never_spends_back_beside_its_registration() {
    let f = fx(3, true);
    let (id, registration, _) = restored_coordinator(&f);
    let r = f.recovery(0, &id);
    assert_eq!(abandon_action(&r), None, "{r:?}");
    assert!(
        r.actions.contains(&RecoveryAction::AbandonWalletTx {
            txid: registration.txid().to_string()
        }),
        "{r:?}"
    );
    assert_stage(f.abandon(0, &id).unwrap_err(), "sent");
    assert_eq!(f.net.sent().len(), 1, "nothing spent back");
}

/// [`restored_coordinator`]: a spend of c2 in a block leaves the
/// registration valid, so it settles nothing and the follower keeps the
/// reservation.
#[test]
fn test_a_restored_draft_is_not_settled_by_a_coin_its_registration_never_spent() {
    let f = fx(3, true);
    let (id, _, c2) = restored_coordinator(&f);
    f.mine_locked(foreign_spend(&[c2], 1), 60);
    f.tip(60);
    f.session.hub.tracker().set_chainlock_height(60);
    let s = f.session_of(0, &id);
    assert_eq!(s.stage, SharedStage::Sent);
    assert_eq!(s.recovery.settlement, Settlement::None);
    f.follow();
    assert!(!reserved_coins(&f, 0, &id).is_empty(), "kept");
}

/// [`restored_coordinator`] with its envelope no longer parsing: read by
/// its columns, a spend of c2 in a block doesn't settle it, so the follower
/// keeps the reservation and the abandon refuses at its out stage, not
/// `conflicted`.
#[test]
fn test_an_unreadable_restored_draft_is_not_settled_by_a_coin_its_registration_never_spent() {
    let f = fx(3, true);
    let (id, _, c2) = restored_coordinator(&f);
    let conn =
        rusqlite::Connection::open(f.session.data_dir().join(dw_appdb::APP_DB_FILE)).unwrap();
    conn.execute(
        "UPDATE shared_mn_sessions SET envelope = '{' WHERE wallet_id = ?1 AND id = ?2",
        rusqlite::params![f.wallets[0].to_string(), id],
    )
    .unwrap();
    f.mine_locked(foreign_spend(&[c2], 1), 60);
    f.tip(60);
    f.session.hub.tracker().set_chainlock_height(60);
    f.follow();
    assert!(!reserved_coins(&f, 0, &id).is_empty(), "kept");
    let e = f
        .run(
            f.mn()
                .abandon_unreadable_shared_session(f.wallets[0], id.clone(), f.grant(0, COIN)),
        )
        .unwrap_err();
    assert_stage(e, "sent");
}

/// A refused copy spending one of our own inputs fifty times, each with our
/// scriptSig, is never ours (no valid transaction spends an outpoint
/// twice): the session stays discardable, not out, and the import checks
/// no more signatures than the session has own inputs, so a copy can't hold
/// the wallet's read for long. Such a copy parses only before `combined`
/// (no consent signature covers its inputs), so it is a `frozen` one.
#[test]
fn test_a_refused_copy_repeating_our_input_stays_discardable() {
    let f = fx(3, true);
    let id = three_approved(&f);
    f.imported(1, &f.message(0, &id));
    let before = f.row(1, &id);
    let signed = f.sign(1, &id, 301 * COIN).unwrap();
    restore(&f, 1, &id, &before);
    let own = reserved_coins(&f, 1, &id);
    let mut copy = parse(&signed.message.text);
    let frozen = decode_frozen(&copy).unwrap();
    let mut tx = frozen.tx;
    let index = (tx.input.iter())
        .position(|i| own.contains(&i.previous_output))
        .unwrap();
    let ours = tx.input[index].clone();
    assert!(!ours.script_sig.is_empty(), "our signature");
    let o = ours.previous_output;
    let listed = (copy.contributions.iter_mut())
        .find_map(|c| {
            let at =
                (c.inputs.iter()).position(|i| i.txid == o.txid.to_string() && i.vout == o.vout)?;
            Some((c, at))
        })
        .unwrap();
    let (contribution, at) = listed;
    let input = contribution.inputs[at].clone();
    for _ in 0..49 {
        tx.input.insert(index + 1, ours.clone());
        contribution.inputs.insert(at + 1, input.clone());
    }
    copy.stage = Stage::Frozen;
    copy.sigs.clear();
    copy.protx = hex::encode(tx.to_bytes());
    copy.consent_hash =
        dw_protx::shared::ProRegTxV3::display_hash(&frozen.payload.consent_hash(&tx));
    copy.revision += 1;
    assert_message(f.import(1, &copy.to_text()).unwrap_err(), "conflict");
    let runs = (SIGNER_RUNS.lock().unwrap().iter())
        .filter(|t| **t == tx.txid())
        .count();
    assert!(runs <= own.len(), "{runs} signature checks");
    let row = f.row(1, &id);
    assert_eq!((row.out_at, row.last_error), (None, None));
    f.spv(true, 1);
    assert!(
        (f.recovery(1, &id).actions.iter()).any(|a| matches!(a, RecoveryAction::Discard { .. })),
        "not out: discard"
    );
}

/// T4 "participant `funding_signed`": with nothing signed back the replay
/// is refused at the out stage; a held reserved coin with no ownership
/// path, though not the largest, blocks the abandon (a named behaviour
/// change), read and call alike.
#[test]
fn test_a_coin_without_an_ownership_path_blocks_the_abandon() {
    let f = fx(3, true);
    let (id, coins) = two_coin_participant(&f);
    f.spv(true, 1);
    assert_stage(
        f.resend_abandon(1, &id, COIN).unwrap_err(),
        "funding_signed",
    );
    assert_eq!(
        f.recovery(1, &id).actions,
        [RecoveryAction::Abandon {
            charge: spend_fee(2),
            spends: true,
            replaces: None
        }]
    );
    let (small, large) = (f.coin_of(1, coins[0]).0, f.coin_of(1, coins[1]).0);
    assert!(small < large);
    f.orphan(1, coins[0]);
    let r = f.recovery(1, &id);
    assert_eq!(abandon_action(&r), None);
    assert_eq!(
        r.blocked.map(|b| b.reason),
        Some(BlockedReason::NoOwnershipPath)
    );
    assert_stage(f.abandon(1, &id).unwrap_err(), "funding_signed");
    assert!(f.net.sent().is_empty());
}

/// T4 "coordinator `sent`, successful hand-off" with the registration
/// recorded: `resend` and Activity's `abandon_wallet_tx{row.txid}`, the
/// abandon refused `sent`; IS-locked, `blocked{row.txid, instant_locked}`;
/// SPV stopped, `blocked{spv_stopped}` and no resend.
#[test]
fn test_a_recorded_registration_lists_resend_and_its_activity_exit() {
    let f = fx(3, true);
    let id = three_approved(&f);
    f.both_signed(&id);
    f.send(0, &id, 400 * COIN + FEE).unwrap();
    let registration = f.net.sent()[0].clone();
    f.record(0, &registration);
    f.spv(true, 1);
    let txid = registration.txid().to_string();
    let debit = f.session_of(0, &id).debit.unwrap();
    let r = f.recovery(0, &id);
    assert_eq!(
        r.actions,
        [
            RecoveryAction::Resend { charge: debit },
            RecoveryAction::AbandonWalletTx { txid: txid.clone() }
        ]
    );
    assert_eq!((r.handoff_error, r.blocked), (None, None));
    assert_stage(f.abandon(0, &id).unwrap_err(), "sent");
    f.history_edit(0, registration.txid(), |e| e.instant_locked = true);
    let r = f.recovery(0, &id);
    assert_eq!(r.actions, [RecoveryAction::Resend { charge: debit }]);
    let b = r.blocked.unwrap();
    assert_eq!(
        (b.txid, b.reason),
        (Some(txid), BlockedReason::InstantLocked)
    );
    f.history_edit(0, registration.txid(), |e| e.instant_locked = false);
    f.spv(false, 0);
    let r = f.recovery(0, &id);
    assert!(r.actions.is_empty());
    assert_eq!(r.blocked.map(|b| b.reason), Some(BlockedReason::SpvStopped));
}

/// T4 "legacy row whose winning conflict isn't in `attributed`": a spend
/// an older build overwrote wins, `settled{other_spend}`.
#[test]
fn test_a_legacy_row_settled_by_an_overwritten_spend_reads_other_spend() {
    let mut f = fx(3, true);
    let (id, coins) = two_coin_participant(&f);
    let a = foreign_spend(&coins[1..], 1);
    f.legacy(1, &id, a.txid(), 1_000);
    f.restart();
    let p = foreign_spend(&coins[..1], 2);
    f.mine_locked(p, 60);
    f.tip(60);
    f.session.hub.tracker().set_chainlock_height(60);
    let r = f.recovery(1, &id);
    assert_eq!(
        r.settlement,
        Settlement::Settled {
            by: SettledBy::OtherSpend,
            released: false
        }
    );
    assert!(r.actions.is_empty());
    assert_eq!(r.abandon_spend.map(|s| s.state), Some(None));
}
