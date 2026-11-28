//! The coin test, the latest abandon spend's state, the hand-off error and
//! the blocked reason, derived from constructed rows, coins and history
//! (MG-10 §4.4 change 2, rules 2, 3 and 9).

use dashcore::{ScriptBuf, Transaction, TxIn, TxOut};
use key_wallet::transaction_checking::{BlockInfo, TransactionContext};

use super::*;

const COIN_UNIT: u64 = 100_000_000;

fn op(seed: u8, vout: u32) -> OutPoint {
    OutPoint::new(Txid::from_byte_array([seed; 32]), vout)
}

fn address() -> dashcore::Address {
    let script = ScriptBuf::new_p2pkh(&dashcore::PubkeyHash::from_byte_array([7; 20]));
    dashcore::Address::from_script(&script, dashcore::Network::Regtest).unwrap()
}

fn path() -> Option<DerivationPath> {
    Some(DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap())
}

fn utxo(outpoint: OutPoint, value: u64) -> Utxo {
    let a = address();
    let out = TxOut {
        value,
        script_pubkey: a.script_pubkey(),
    };
    Utxo::new(outpoint, out, a, 1, false)
}

/// The facts of a session reserving `reserved`, the wallet holding `held`
/// (each with its ownership path), nothing claimed or signed.
fn facts(reserved: &[OutPoint], held: &[(OutPoint, u64)]) -> CoinFacts {
    let mut held: Vec<_> = held.iter().map(|(o, v)| (utxo(*o, *v), path())).collect();
    held.sort_by_key(|(u, _)| u.outpoint);
    CoinFacts {
        reserved: reserved.to_vec(),
        held,
        excluded: Default::default(),
        pending: HashSet::new(),
        own: Vec::new(),
        latest: None,
        signed: None,
    }
}

fn spending(inputs: &[OutPoint], tag: u64) -> Transaction {
    Transaction {
        version: 3,
        lock_time: 0,
        input: inputs
            .iter()
            .map(|o| TxIn {
                previous_output: *o,
                ..Default::default()
            })
            .collect(),
        output: vec![TxOut {
            value: tag,
            script_pubkey: ScriptBuf::new(),
        }],
        special_transaction_payload: None,
    }
}

fn entry(tx: Transaction, context: TransactionContext, abandoned: bool, locked: bool) -> TxEntry {
    TxEntry {
        tx,
        context,
        instant_locked: locked,
        first_seen: None,
        own_inputs: Default::default(),
        abandoned,
    }
}

fn mempool(tx: Transaction) -> TxEntry {
    entry(tx, TransactionContext::Mempool, false, false)
}

fn in_block(tx: Transaction) -> TxEntry {
    let info = BlockInfo::new(10, dashcore::BlockHash::all_zeros(), 0);
    entry(tx, TransactionContext::InBlock(info), false, false)
}

fn history(entries: impl IntoIterator<Item = TxEntry>) -> HashMap<Txid, TxEntry> {
    entries.into_iter().map(|e| (e.tx.txid(), e)).collect()
}

fn usable_fee(coins: &AbandonCoins) -> u64 {
    match coins {
        AbandonCoins::Usable { fee, .. } => *fee,
        other => panic!("not usable: {other:?}"),
    }
}

/// Change 2's first match: a claim on a held coin comes before a missing
/// ownership path, which blocks the spend even on a coin that isn't the
/// largest (a named behaviour change).
#[test]
fn the_coin_test_checks_claims_then_ownership_paths() {
    let (a, b) = (op(1, 0), op(2, 0));
    let mut f = facts(&[a, b], &[(a, 5 * COIN_UNIT), (b, COIN_UNIT)]);
    f.held[1].1 = None;
    assert_eq!(
        f.test(&HashMap::new()),
        AbandonCoins::NoOwnershipPath { outpoint: b }
    );
    f.pending.insert(a);
    assert_eq!(f.test(&HashMap::new()), AbandonCoins::HeldInFlight);
}

/// With no reserved coin held: the session's own transaction, then the
/// latest abandon spend, then the lowest spender of the first reserved
/// outpoint; abandoned and confirmed spenders are never blockers.
#[test]
fn the_blocker_lookup_order() {
    let (a, b) = (op(1, 0), op(2, 0));
    let own = spending(&[b], 1);
    let spend_back = spending(&[b], 2);
    let other_a = spending(&[a], 3);
    let other_b = spending(&[b], 4);
    let mut f = facts(&[a, b], &[]);
    f.own = vec![own.txid()];
    f.latest = Some((Some(spend_back.txid()), vec![a, b]));
    let all = || {
        history([
            mempool(own.clone()),
            mempool(spend_back.clone()),
            mempool(other_a.clone()),
            mempool(other_b.clone()),
        ])
    };
    let blocked = |h: &HashMap<Txid, TxEntry>| match f.test(h) {
        AbandonCoins::BlockedByWalletTx { txid } => Some(txid),
        AbandonCoins::Unavailable => None,
        other => panic!("{other:?}"),
    };
    let mut h = all();
    assert_eq!(blocked(&h), Some(own.txid()));
    h.remove(&own.txid());
    assert_eq!(blocked(&h), Some(spend_back.txid()));
    h.remove(&spend_back.txid());
    assert_eq!(blocked(&h), Some(other_a.txid()), "the first outpoint");
    h.get_mut(&other_a.txid()).unwrap().abandoned = true;
    assert_eq!(blocked(&h), Some(other_b.txid()), "abandoned: skipped");
    let confirmed = history([in_block(other_b.clone())]);
    assert_eq!(blocked(&confirmed), None, "confirmed: skipped");
    // Among the spenders of one outpoint, the smallest txid.
    let twin = spending(&[a], 5);
    let h = history([mempool(other_a.clone()), mempool(twin.clone())]);
    assert_eq!(blocked(&h), Some(other_a.txid().min(twin.txid())));
}

/// R1: with a spend back signed, the held coins must include its inputs;
/// the first live, not abandoned spender of a missing one is named.
#[test]
fn the_inclusion_bound() {
    let (a, b, c) = (op(1, 0), op(2, 0), op(3, 0));
    let mut f = facts(&[a, b, c], &[(a, COIN_UNIT), (c, COIN_UNIT)]);
    assert!(
        matches!(f.test(&HashMap::new()), AbandonCoins::Usable { ref coins, .. } if coins.len() == 2),
        "nothing signed: no bound"
    );
    f.latest = Some((Some(op(9, 0).txid), vec![a, b]));
    assert_eq!(
        f.test(&HashMap::new()),
        AbandonCoins::Unbounded {
            missing: vec![b],
            spender: None
        }
    );
    let x = spending(&[b], 1);
    let mut h = history([mempool(x.clone())]);
    assert_eq!(
        f.test(&h),
        AbandonCoins::Unbounded {
            missing: vec![b],
            spender: Some(x.txid())
        }
    );
    h.get_mut(&x.txid()).unwrap().abandoned = true;
    assert_eq!(
        f.test(&h),
        AbandonCoins::Unbounded {
            missing: vec![b],
            spender: None
        },
        "an abandoned spender is never named"
    );
    f.latest = Some((Some(op(9, 0).txid), vec![a]));
    match f.test(&HashMap::new()) {
        AbandonCoins::Usable { coins, .. } => {
            let taken: Vec<OutPoint> = coins.iter().map(|(u, _)| u.outpoint).collect();
            assert_eq!(taken, [a, c], "every held reserved coin, outpoint order");
        }
        other => panic!("{other:?}"),
    }
}

/// The value check: worth no more than the n-input fee plus dust is
/// `TooSmall`; one duff more is `Usable`.
#[test]
fn the_value_check_boundary_and_the_n_input_fee() {
    let a = op(1, 0);
    let dust = dust_threshold(CHANGE_SCRIPT_LEN);
    let fee = spend_fee(1);
    let f = facts(&[a], &[(a, fee + dust)]);
    assert_eq!(
        f.test(&HashMap::new()),
        AbandonCoins::TooSmall {
            fee,
            available: fee + dust
        }
    );
    let f = facts(&[a], &[(a, fee + dust + 1)]);
    assert_eq!(usable_fee(&f.test(&HashMap::new())), fee);
    let f = facts(&[a], &[(a, fee - 1)]);
    assert!(matches!(
        f.test(&HashMap::new()),
        AbandonCoins::TooSmall { .. }
    ));
    let b = op(2, 0);
    let f = facts(&[a, b], &[(a, COIN_UNIT), (b, COIN_UNIT)]);
    assert_eq!(usable_fee(&f.test(&HashMap::new())), spend_fee(2));
    assert!(spend_fee(2) > spend_fee(1));
    assert_eq!(
        spend_fee(2),
        fee_for(FeeRate::new(MIN_FEE_PER_KB), tx_size(2, &[25], 0)).unwrap()
    );
}

fn row() -> SharedSessionRow {
    SharedSessionRow {
        id: "00".repeat(32),
        kind: "registration".into(),
        role: "coordinator".into(),
        envelope: "{}".into(),
        my_shares: "{}".into(),
        contribution: None,
        reserved: "[]".into(),
        approved_terms: None,
        base_keys: None,
        signed_tx: None,
        txid: None,
        out_at: Some(1),
        abandon_txid: None,
        created_at: 1,
        updated_at: 1,
        last_error: None,
        abandon_tx: None,
        abandon_log: None,
        legacy_bound: false,
        resend_marked: false,
    }
}

fn log_entry(txid: &str, outcome: Option<&str>, code: Option<&str>) -> LogEntry {
    LogEntry {
        txid: txid.into(),
        fee: Some(1),
        inputs: Vec::new(),
        signed_at: 1,
        outcome: outcome.map(str::to_string),
        code: code.map(str::to_string),
        legacy: false,
    }
}

/// Rule 2, first match: in history (abandoned there or not; a lock
/// outranks the mark), a durable refusal, stored bytes, else legacy. A
/// spend back in a block is never read here.
#[test]
fn the_spend_back_state() {
    let a = spending(&[op(1, 0)], 1);
    let txid = a.txid().to_string();
    let mut r = row();
    assert_eq!(spend_state(&r, None, &HashMap::new()), None);
    r.abandon_txid = Some(txid.clone());
    assert_eq!(
        spend_state(&r, None, &HashMap::new()),
        Some(AbandonSpendState::Legacy)
    );
    r.abandon_tx = Some(dashcore::consensus::serialize(&a));
    for outcome in [None, Some("sent"), Some("maybe_sent")] {
        let e = log_entry(&txid, outcome, None);
        assert_eq!(
            spend_state(&r, Some(&e), &HashMap::new()),
            Some(AbandonSpendState::Uncertain),
            "{outcome:?}"
        );
    }
    let refused = log_entry(&txid, Some("not_sent"), Some("send.no_peers"));
    assert_eq!(
        spend_state(&r, Some(&refused), &HashMap::new()),
        Some(AbandonSpendState::Refused)
    );
    let recorded = history([mempool(a.clone())]);
    assert_eq!(
        spend_state(&r, Some(&refused), &recorded),
        Some(AbandonSpendState::InHistory),
        "the wallet's record outranks the stored outcome"
    );
    let abandoned = history([entry(a.clone(), TransactionContext::Mempool, true, false)]);
    assert_eq!(
        spend_state(&r, None, &abandoned),
        Some(AbandonSpendState::AbandonedInActivity)
    );
    let locked = history([entry(a.clone(), TransactionContext::Mempool, true, true)]);
    assert_eq!(
        spend_state(&r, None, &locked),
        Some(AbandonSpendState::InHistory)
    );
    let mined = history([in_block(a)]);
    assert_eq!(
        spend_state(&r, None, &mined),
        Some(AbandonSpendState::Uncertain)
    );
}

/// Rule 3: the session's own hand-off reads `refused` only for a refusal
/// code with no resend marked; a spend back's only from its entry's code.
#[test]
fn the_hand_off_error() {
    let mut r = row();
    assert_eq!(handoff_error(&r, None), None);
    r.last_error = Some("send.broadcast_rejected".into());
    let e = handoff_error(&r, None).unwrap();
    assert_eq!(
        (e.tx, e.outcome, e.resend_refused),
        (HandoffTx::Registration, HandoffErrorOutcome::Refused, false)
    );
    r.resend_marked = true;
    let e = handoff_error(&r, None).unwrap();
    assert_eq!(
        (e.outcome, e.resend_refused),
        (HandoffErrorOutcome::Unknown, true)
    );
    r.last_error = Some(dw_appdb::RESEND_IN_PROGRESS.into());
    let e = handoff_error(&r, None).unwrap();
    assert_eq!(
        (e.outcome, e.resend_refused, e.code.as_deref()),
        (
            HandoffErrorOutcome::Unknown,
            false,
            Some(dw_appdb::RESEND_IN_PROGRESS)
        )
    );
    r.resend_marked = false;
    r.last_error = Some("send.broadcast_unknown".into());
    assert_eq!(
        handoff_error(&r, None).unwrap().outcome,
        HandoffErrorOutcome::Unknown
    );

    r.abandon_txid = Some("ab".into());
    assert_eq!(handoff_error(&r, None), None, "no entry: no reason");
    let quiet = log_entry("ab", Some("maybe_sent"), None);
    assert_eq!(handoff_error(&r, Some(&quiet)), None, "no code: no reason");
    let refused = log_entry("ab", Some("not_sent"), Some("send.no_peers"));
    let e = handoff_error(&r, Some(&refused)).unwrap();
    assert_eq!(
        (e.tx, e.outcome, e.code.as_deref()),
        (
            HandoffTx::Abandon,
            HandoffErrorOutcome::Refused,
            Some("send.no_peers")
        )
    );
    let unknown = log_entry("ab", Some("maybe_sent"), Some("send.broadcast_unknown"));
    assert_eq!(
        handoff_error(&r, Some(&unknown)).unwrap().outcome,
        HandoffErrorOutcome::Unknown
    );
}

fn reason(b: Option<Blocked>) -> Option<(Option<String>, BlockedReason)> {
    b.map(|b| (b.txid, b.reason))
}

/// Rule 9, first match: SPV stopped, an InstantSend lock, then, only where
/// a new spend is what the spend back's state allows, the coin test's arm,
/// with an abandoned transaction's missing coin read as the rescan's.
#[test]
fn the_blocked_reason() {
    let (a, b) = (op(1, 0), op(2, 0));
    let x = spending(&[b], 1);
    let spend_back = spending(&[a, b], 2);
    let f = facts(&[a, b], &[(a, COIN_UNIT)]);
    let unbounded = AbandonCoins::Unbounded {
        missing: vec![b],
        spender: Some(x.txid()),
    };
    let h = history([mempool(x.clone())]);
    let back = Some(spend_back.txid());
    let run = |coins: &AbandonCoins, h: &HashMap<Txid, TxEntry>, new: bool, spv, locked| {
        reason(blocked(&f, coins, h, &[], back, new, spv, locked))
    };
    assert_eq!(
        run(&unbounded, &h, false, true, Some(x.txid())),
        Some((None, BlockedReason::SpvStopped))
    );
    assert_eq!(
        run(&unbounded, &h, false, false, Some(x.txid())),
        Some((Some(x.txid().to_string()), BlockedReason::InstantLocked))
    );
    assert_eq!(run(&unbounded, &h, false, false, None), None);
    assert_eq!(
        run(&unbounded, &h, true, false, None),
        Some((Some(x.txid().to_string()), BlockedReason::CoinElsewhere))
    );
    assert_eq!(
        run(&AbandonCoins::HeldInFlight, &h, true, false, None),
        Some((None, BlockedReason::InFlight))
    );
    assert_eq!(
        run(
            &AbandonCoins::NoOwnershipPath { outpoint: a },
            &h,
            true,
            false,
            None
        ),
        Some((None, BlockedReason::NoOwnershipPath))
    );
    let small = AbandonCoins::TooSmall {
        fee: 1,
        available: 1,
    };
    assert_eq!(
        run(&small, &h, true, false, None),
        Some((None, BlockedReason::TooSmall))
    );

    // The missing coin is an input of a transaction abandoned in Activity
    // that the session signed: the rescan's, not elsewhere.
    let mut f2 = facts(&[a, b], &[(a, COIN_UNIT)]);
    f2.latest = Some((Some(spend_back.txid()), vec![a, b]));
    let abandoned = history([entry(
        spend_back.clone(),
        TransactionContext::Mempool,
        true,
        false,
    )]);
    let gone = AbandonCoins::Unbounded {
        missing: vec![b],
        spender: None,
    };
    let read = |f: &CoinFacts, h| reason(blocked(f, &gone, h, &[], back, true, false, None));
    assert_eq!(
        read(&f2, &abandoned),
        Some((
            Some(spend_back.txid().to_string()),
            BlockedReason::Rescanning
        ))
    );
    f2.pending.insert(b);
    assert_eq!(
        read(&f2, &abandoned),
        Some((None, BlockedReason::CoinElsewhere)),
        "a claimed reserved coin is never the rescan's"
    );

    // A blocker that is neither the session's own nor the spend back is
    // elsewhere; the session's own is Activity's exit, not blocked.
    let none_held = facts(&[a, b], &[]);
    let foreign = AbandonCoins::BlockedByWalletTx { txid: x.txid() };
    assert_eq!(
        reason(blocked(
            &none_held,
            &foreign,
            &h,
            &[],
            back,
            true,
            false,
            None
        )),
        Some((Some(x.txid().to_string()), BlockedReason::CoinElsewhere))
    );
    let mut own = facts(&[a, b], &[]);
    own.own = vec![x.txid()];
    assert_eq!(
        reason(blocked(&own, &foreign, &h, &[], back, true, false, None)),
        None
    );
    assert_eq!(
        reason(blocked(
            &none_held,
            &foreign,
            &h,
            &[],
            None,
            true,
            false,
            None
        )),
        None,
        "no spend back signed: not elsewhere"
    );
}

#[test]
fn settled_stages() {
    assert!(settled_stage(SharedStage::Conflicted, false));
    assert!(settled_stage(SharedStage::Confirmed { height: 1 }, false));
    assert!(!settled_stage(SharedStage::Sent, true));
    let dead = SharedStage::Dead {
        reason: crate::masternodes::DeadReason::Abandoned,
    };
    assert!(!settled_stage(dead, false));
    assert!(settled_stage(dead, true));
}
