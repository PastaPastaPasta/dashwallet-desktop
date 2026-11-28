//! Durable sent marks on `broadcast_psbt`: before handing a finalized PSBT to
//! peers, each owning wallet records `sent:`, `senttx:`, and `sentin:` under
//! its coin fence so a crash before key-wallet's mempool record commits never
//! re-offers or re-counts the spent inputs after restart. Marks written by the
//! call are dropped on `NoPeers` / `BroadcastRejected` and kept on success or
//! `BroadcastUnknown`.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use dashcore::{OutPoint, Transaction, TxIn, TxOut, Txid};
use dw_psbt::PartiallySignedTransaction;
use dw_vault::{Credential, GrantPurpose};
use platform_wallet::broadcaster::BroadcastError;

use super::own_spend_disk_tests::{COIN, assert_holds, spendable, wallet_funded_unmined};
use super::{elsewhere, reopen, rows};
use crate::send::psbt::PsbtBroadcaster;
use crate::session::NetworkSession;
use crate::{Engine, EngineError, PsbtFailure, WalletId};

struct ScriptedBroadcaster {
    seen_mark_before_broadcast: Arc<AtomicBool>,
    session: Weak<NetworkSession>,
    wallet: WalletId,
    outcome: Mutex<Option<Result<(), BroadcastError>>>,
}

impl PsbtBroadcaster for ScriptedBroadcaster {
    fn broadcast(
        &self,
        tx: &Transaction,
    ) -> Pin<Box<dyn Future<Output = Result<Txid, BroadcastError>> + Send>> {
        let txid = tx.txid();
        let session = self.session.upgrade().expect("session alive");
        let (sent, read_ok) =
            crate::tx_actions::load_sent(&session.live().unwrap().appdb, self.wallet);
        let has_mark = read_ok
            && sent.get(&txid).is_some_and(|m| {
                m.tx.as_ref() == Some(tx) && !m.coins.is_empty() && !m.inputs.is_empty()
            });
        self.seen_mark_before_broadcast
            .store(has_mark, Ordering::SeqCst);
        let next = self
            .outcome
            .lock()
            .unwrap()
            .take()
            .expect("scripted broadcast outcome");
        Box::pin(async move { next.map(|()| txid) })
    }
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

fn assert_no_sent_keys(session: &NetworkSession, id: WalletId) {
    let appdb = &session.live().unwrap().appdb;
    let scope = id.to_string();
    for prefix in [
        crate::tx_actions::SENT_PREFIX,
        crate::tx_actions::SENT_TX_PREFIX,
        crate::tx_actions::SENT_IN_PREFIX,
    ] {
        let rows = appdb.settings_with_prefix(&scope, prefix).unwrap();
        assert!(
            rows.is_empty(),
            "expected no {prefix} keys remaining, got {rows:?}"
        );
    }
}

fn signed_psbt_spending(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    id: WalletId,
    paid: &Transaction,
    vout: u32,
) -> (PartiallySignedTransaction, OutPoint) {
    let outpoint = OutPoint::new(paid.txid(), vout);
    let unsigned = Transaction {
        version: 2,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: outpoint,
            ..Default::default()
        }],
        output: vec![TxOut {
            value: paid.output[vout as usize].value - 1_000,
            script_pubkey: elsewhere(),
        }],
        special_transaction_payload: None,
    };
    let mut psbt = PartiallySignedTransaction::from_unsigned_tx(unsigned).unwrap();
    psbt.inputs[0].non_witness_utxo = Some(paid.clone());
    let grant = session
        .vault()
        .authorize(
            GrantPurpose::Spend {
                max_duffs: paid.output[vout as usize].value,
            },
            Some(&id.0),
            Credential::None,
        )
        .unwrap()
        .id;
    let signed = engine
        .block_on(session.sign_psbt(id, psbt, grant))
        .expect("sign PSBT");
    (signed, outpoint)
}

fn install_broadcaster(
    session: &Arc<NetworkSession>,
    id: WalletId,
    outcome: Result<(), BroadcastError>,
) -> Arc<AtomicBool> {
    let seen = Arc::new(AtomicBool::new(false));
    *session.psbt_broadcaster.lock().unwrap() = Some(Arc::new(ScriptedBroadcaster {
        seen_mark_before_broadcast: Arc::clone(&seen),
        session: Arc::downgrade(session),
        wallet: id,
        outcome: Mutex::new(Some(outcome)),
    }));
    seen
}

#[test]
fn psbt_broadcast_keeps_input_out_across_restart_without_mempool_record() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (signed, a) = signed_psbt_spending(&engine, &session, id, &paid, 0);
    let seen = install_broadcaster(&session, id, Ok(()));

    let txid = engine
        .block_on(session.broadcast_psbt(signed))
        .expect("broadcast_psbt succeeds via broadcaster seam");
    assert!(
        seen.load(Ordering::SeqCst),
        "sent mark (sent:/senttx:/sentin:) must be durable before broadcast"
    );
    assert!(
        sent_marks(&session, id).contains(&txid),
        "sent mark must remain after successful broadcast"
    );

    // Simulate dying before key-wallet's mempool record commits: reopen
    // straight from disk without feeding `txid` through SPV.
    let session = reopen(&engine, session);
    let mut want = outputs(&paid);
    want.remove(&a);
    assert!(
        sent_marks(&session, id).contains(&txid),
        "sent mark must survive restart"
    );
    assert!(
        rows(&engine, &session, id)
            .iter()
            .any(|r| r.0 == txid.to_string()),
        "PSBT transaction must be listed pending after restart"
    );
    let b = assert_holds(&engine, &session, id, &want, "after PSBT restart");
    assert_eq!(
        b.total,
        (want.len() as u64) * COIN,
        "spent PSBT input must not be counted in available balance after restart"
    );
    assert!(
        !spendable(&engine, &session, id, a),
        "spent PSBT input must not be offered after restart"
    );
    let extras = engine
        .block_on(session.tx_detail_extras(id, txid))
        .expect("tx_detail_extras for restored PSBT send");
    assert!(
        extras.can_abandon && extras.can_resend,
        "restored PSBT send must offer Abandon and Resend: {extras:?}"
    );
}

#[test]
fn psbt_broadcast_drops_sent_mark_on_no_peers_and_rejected() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (signed, a) = signed_psbt_spending(&engine, &session, id, &paid, 0);

    // Case 1: NoPeers — mark is written before broadcast and dropped on NoPeers.
    let seen_no_peers = install_broadcaster(
        &session,
        id,
        Err(BroadcastError::Rejected {
            reason: "no peers connected".into(),
        }),
    );
    let err = engine
        .block_on(session.broadcast_psbt(signed.clone()))
        .expect_err("NoPeers must fail broadcast_psbt");
    assert!(
        matches!(err, EngineError::Psbt(PsbtFailure::NoPeers)),
        "expected PsbtFailure::NoPeers, got {err:?}"
    );
    assert!(
        seen_no_peers.load(Ordering::SeqCst),
        "sent mark must be written before calling broadcaster"
    );
    assert_no_sent_keys(&session, id);

    let session = reopen(&engine, session);
    let all = outputs(&paid);
    assert_no_sent_keys(&session, id);
    assert_holds(&engine, &session, id, &all, "after NoPeers restart");
    assert!(
        spendable(&engine, &session, id, a),
        "input must stay spendable after NoPeers"
    );

    // Case 2: BroadcastRejected — mark is also written before broadcast and dropped.
    let seen_rejected = install_broadcaster(
        &session,
        id,
        Err(BroadcastError::Rejected {
            reason: "txn-mempool-conflict".into(),
        }),
    );
    let err = engine
        .block_on(session.broadcast_psbt(signed))
        .expect_err("Rejected must fail broadcast_psbt");
    assert!(
        matches!(
            err,
            EngineError::Psbt(PsbtFailure::BroadcastRejected { .. })
        ),
        "expected PsbtFailure::BroadcastRejected, got {err:?}"
    );
    assert!(
        seen_rejected.load(Ordering::SeqCst),
        "sent mark must be written before calling broadcaster"
    );
    assert_no_sent_keys(&session, id);

    let session = reopen(&engine, session);
    assert_no_sent_keys(&session, id);
    assert_holds(&engine, &session, id, &all, "after Rejected restart");
    assert!(
        spendable(&engine, &session, id, a),
        "input must stay spendable after BroadcastRejected"
    );
}

#[test]
fn psbt_broadcast_unknown_keeps_sent_mark_across_restart_and_retry_no_peers() {
    let (_dir, engine, session, id, paid) = wallet_funded_unmined();
    let (signed, a) = signed_psbt_spending(&engine, &session, id, &paid, 0);
    let mut finalized = signed.clone();
    assert!(dw_psbt::finalize(&mut finalized));
    let txid = dw_psbt::extract(&finalized).unwrap().txid();

    let seen_unknown = install_broadcaster(
        &session,
        id,
        Err(BroadcastError::MaybeSent {
            reason: "timed out waiting for peer acceptance".into(),
        }),
    );
    let err = engine
        .block_on(session.broadcast_psbt(signed.clone()))
        .expect_err("MaybeSent must return BroadcastUnknown");
    assert!(
        matches!(err, EngineError::Psbt(PsbtFailure::BroadcastUnknown { .. })),
        "expected PsbtFailure::BroadcastUnknown, got {err:?}"
    );
    assert!(seen_unknown.load(Ordering::SeqCst));
    assert!(
        sent_marks(&session, id).contains(&txid),
        "BroadcastUnknown must keep the sent mark"
    );

    // A later retry that hits NoPeers must NOT erase the pre-existing mark
    // from the earlier MaybeSent attempt.
    let _seen_retry = install_broadcaster(
        &session,
        id,
        Err(BroadcastError::Rejected {
            reason: "no peers connected".into(),
        }),
    );
    let err = engine
        .block_on(session.broadcast_psbt(signed))
        .expect_err("retry with NoPeers must return NoPeers");
    assert!(matches!(err, EngineError::Psbt(PsbtFailure::NoPeers)));
    assert!(
        sent_marks(&session, id).contains(&txid),
        "later NoPeers retry must not drop an existing sent mark"
    );

    let session = reopen(&engine, session);
    let mut want = outputs(&paid);
    want.remove(&a);
    assert!(sent_marks(&session, id).contains(&txid));
    assert_holds(
        &engine,
        &session,
        id,
        &want,
        "after BroadcastUnknown restart",
    );
    assert!(
        !spendable(&engine, &session, id, a),
        "input must remain held after BroadcastUnknown restart"
    );
}
