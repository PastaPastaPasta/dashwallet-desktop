//! BL-76 (E0-04 H12, §4.6, §5.5): the wallet's positive network evidence
//! reaches the fence through the session hub, a call step's Ok announces
//! `Sent`, and what one wallet's change resolves for another wallet is
//! announced to it. On the table and a hub with real wallet events.

use std::collections::BTreeMap;
use std::sync::Arc;

use dashcore::bls_sig_utils::BLSSignature;
use dashcore::hashes::Hash;
use dashcore::{BlockHash, ChainLock, InstantLock, OutPoint, Transaction, TxIn, Txid};
use key_wallet::account::{AccountType, StandardAccountType};
use key_wallet::managed_account::transaction_record::{TransactionDirection, TransactionRecord};
use key_wallet::transaction_checking::{BlockInfo, TransactionContext, TransactionType};
use platform_wallet::events::WalletEvent;

use super::tests::{Recorder, W, W2, begin, core, credits, draft, first, table, with_journal};
use super::*;
use crate::events::SessionHub;
use crate::platform::flows::{DispatchResolution, DispatchState, RevokeCause};
use crate::{DashNetwork, WalletId};

/// Every `DispatchResolved` as (wallet, artifact, resolution).
fn resolved(rec: &Recorder) -> Vec<(WalletId, String, DispatchResolution)> {
    rec.events()
        .into_iter()
        .filter_map(|e| match e {
            crate::EngineEvent::DispatchResolved { resolved, .. } => Some((
                resolved.wallet_id.parse().unwrap(),
                resolved.artifact,
                resolved.resolution,
            )),
            _ => None,
        })
        .collect()
}

fn ev(w: WalletId, a: ArtifactId, r: DispatchResolution) -> (WalletId, String, DispatchResolution) {
    (w, a.to_string(), r)
}

use DispatchResolution::{NotSent, Sent};

/// `wallet`'s tombstone of `a`: its First definitely failed.
async fn tombstone(l: &Lease, a: ArtifactId) {
    draft(l, a, 100);
    let p = first(l.table.admit(core(l, a)).await);
    assert_eq!(p.finish(Outcome::NotSent), Settlement::DefinitelyUnsent);
}

/// §5.5: a tombstone reads `MaybeSent` while another wallet holds the same
/// bytes. A host that read it hears `NotSent` again when the hold ends, and
/// `Sent` when another wallet's sighting makes the bytes sent; a tombstone
/// nobody read during the hold keeps its own `NotSent` silently.
#[tokio::test(start_paused = true)]
async fn another_wallets_hold_and_sighting_are_announced() {
    let (t, rec) = table();
    with_journal(&t);
    let l1 = begin(&t, W, 0, 0, 1_000).await.unwrap();
    let l2 = begin(&t, W2, 0, 0, 1_000).await.unwrap();
    let (a, b, c) = (tests::art(5), tests::art(6), tests::art(7));

    // The hold ends unsent.
    tombstone(&l2, a).await;
    draft(&l1, a, 100);
    let p = first(t.admit(core(&l1, a)).await);
    assert_eq!(t.dispatch_status(W2, a), Some(DispatchState::MaybeSent));
    assert_eq!(resolved(&rec), vec![ev(W2, a, NotSent)]);
    assert_eq!(p.finish(Outcome::NotSent), Settlement::DefinitelyUnsent);
    assert_eq!(t.dispatch_status(W2, a), Some(DispatchState::NotSent));
    assert_eq!(
        resolved(&rec),
        vec![ev(W2, a, NotSent), ev(W, a, NotSent), ev(W2, a, NotSent)],
        "W2's MaybeSent ended"
    );

    // The held bytes are seen: Sent for both, once each.
    let before = resolved(&rec).len();
    tombstone(&l2, b).await;
    draft(&l1, b, 100);
    let p = first(t.admit(core(&l1, b)).await);
    assert_eq!(t.dispatch_status(W2, b), Some(DispatchState::MaybeSent));
    assert_eq!(p.finish(Outcome::MaybeSent), Settlement::MaybeOut);
    assert_eq!(t.dispatch_status(W2, b), Some(DispatchState::MaybeSent));
    t.note_seen(W, b);
    t.note_seen(W, b);
    assert_eq!(t.dispatch_status(W2, b), Some(DispatchState::Sent));
    assert_eq!(
        resolved(&rec)[before..],
        [ev(W2, b, NotSent), ev(W, b, Sent), ev(W2, b, Sent)]
    );

    // Nobody read W2's MaybeSent: its NotSent stays without a second event.
    let before = resolved(&rec).len();
    tombstone(&l2, c).await;
    draft(&l1, c, 100);
    let p = first(t.admit(core(&l1, c)).await);
    assert_eq!(p.finish(Outcome::NotSent), Settlement::DefinitelyUnsent);
    assert_eq!(
        resolved(&rec)[before..],
        [ev(W2, c, NotSent), ev(W, c, NotSent)]
    );
}

/// The lock report reads a held tombstone's outcome as `MaybeSent`, so its
/// wallet hears how the hold ends.
#[tokio::test(start_paused = true)]
async fn a_lock_report_of_a_held_tombstone_counts_as_a_read() {
    let (t, rec) = table();
    with_journal(&t);
    let l1 = begin(&t, W, 0, 0, 1_000).await.unwrap();
    let l2 = begin(&t, W2, 0, 0, 1_000).await.unwrap();
    let a = tests::art(5);
    tombstone(&l2, a).await;
    draft(&l1, a, 100);
    let p = first(t.admit(core(&l1, a)).await);
    assert_eq!(p.finish(Outcome::MaybeSent), Settlement::MaybeOut);
    assert!(!t.inspect(|i| i.fence.hold_read(&W2, &a)));
    let report = t.lock(RevokeCause::Lock, tests::status).await;
    let flow = report
        .flows
        .iter()
        .find(|f| f.lease == l2.id_string())
        .unwrap();
    assert_eq!(flow.outcome, FlowOutcome::MaybeSent);
    assert!(t.inspect(|i| i.fence.hold_read(&W2, &a)));
    t.note_seen(W, a);
    assert!(!t.inspect(|i| i.fence.hold_read(&W2, &a)));
    assert_eq!(
        resolved(&rec),
        vec![ev(W2, a, NotSent), ev(W, a, Sent), ev(W2, a, Sent)]
    );
}

/// A resumable step's call that returned Ok holds the proved execution:
/// `DispatchResolved(Sent)` under the step's id, once, after the possibly
/// out call before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resumable_steps_ok_call_announces_sent_once() {
    const SUBMIT: &str = "withdrawal/w/submit";
    let (t, rec) = table();
    with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let step = || CallStep::Resumable(SUBMIT.into());
    let p = l
        .call_permit_step("submit", credits(10), step())
        .await
        .unwrap();
    p.signatures().note_released();
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    assert!(resolved(&rec).is_empty());
    let p = l
        .call_permit_step("submit", credits(10), step())
        .await
        .unwrap();
    assert_eq!(p.finish(true), FlowOutcome::Sent);
    assert_eq!(
        resolved(&rec),
        vec![(W, SUBMIT.to_owned(), Sent)],
        "under the step's id"
    );
    // Sent already: a further Ok announces nothing new.
    let p = l
        .call_permit_step("submit", credits(10), step())
        .await
        .unwrap();
    assert_eq!(p.finish(true), FlowOutcome::Sent);
    assert_eq!(resolved(&rec).len(), 1);
}

fn tx(n: u32) -> Transaction {
    Transaction {
        version: 2,
        lock_time: n,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([n as u8; 32]), 0),
            ..Default::default()
        }],
        output: Vec::new(),
        special_transaction_payload: None,
    }
}

fn record(tx: &Transaction, context: TransactionContext) -> TransactionRecord {
    TransactionRecord::new(
        tx.clone(),
        AccountType::Standard {
            index: 0,
            standard_account_type: StandardAccountType::BIP44Account,
        },
        context,
        TransactionType::Standard,
        TransactionDirection::Outgoing,
        Vec::new(),
        Vec::new(),
        -1_000,
    )
}

fn block(height: u32) -> BlockInfo {
    BlockInfo::new(height, BlockHash::all_zeros(), 0)
}

fn detected(w: WalletId, r: TransactionRecord) -> WalletEvent {
    WalletEvent::TransactionDetected {
        wallet_id: w.0,
        record: Box::new(r),
        balance: Default::default(),
        account_balances: BTreeMap::new(),
        addresses_derived: Vec::new(),
    }
}

fn in_block(
    w: WalletId,
    inserted: Vec<TransactionRecord>,
    updated: Vec<TransactionRecord>,
) -> WalletEvent {
    WalletEvent::BlockProcessed {
        wallet_id: w.0,
        height: 10,
        chain_lock: None,
        inserted,
        updated,
        matured: Vec::new(),
        balance: Default::default(),
        account_balances: BTreeMap::new(),
        addresses_derived: Vec::new(),
    }
}

/// A leased First cut as `MaybeOut` (possibly out) for `wallet`: its
/// status reads `MaybeSent`.
async fn maybe_out(l: &Lease, tx: &Transaction) -> ArtifactId {
    let a = ArtifactId::from_txid(&tx.txid());
    draft(l, a, 100);
    let p = first(l.table.admit(core(l, a)).await);
    assert_eq!(p.finish(Outcome::MaybeSent), Settlement::MaybeOut);
    assert_eq!(
        l.table.dispatch_status(l.wallet(), a),
        Some(DispatchState::MaybeSent)
    );
    a
}

/// The session hub feeds the fence the wallet's positive network evidence
/// (BL-76): a block that holds the transaction (inserted, or confirmed
/// now), its InstantSend lock (also when the wallet first sees it through
/// the lock) and a ChainLock over it. A mempool sighting is none: the
/// wallet records its own transactions before any peer took them. Another
/// wallet's evidence says nothing of this wallet's entry.
#[tokio::test(start_paused = true)]
async fn the_hub_feeds_positive_network_evidence_only() {
    let (t, rec) = table();
    with_journal(&t);
    let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(Recorder::default()));
    hub.attach_leases(&t);
    let l = begin(&t, W, 0, 0, 10_000).await.unwrap();
    let status = |a| t.dispatch_status(W, a);

    // Mempool, then a block confirming it.
    let (t1, t2, t3, t4, t5) = (tx(1), tx(2), tx(3), tx(4), tx(5));
    let a1 = maybe_out(&l, &t1).await;
    hub.apply_wallet_event(&detected(W, record(&t1, TransactionContext::Mempool)));
    assert_eq!(status(a1), Some(DispatchState::MaybeSent), "mempool");
    hub.apply_wallet_event(&in_block(
        W2,
        vec![record(&t1, TransactionContext::InBlock(block(10)))],
        Vec::new(),
    ));
    assert_eq!(
        status(a1),
        Some(DispatchState::MaybeSent),
        "another wallet's"
    );
    hub.apply_wallet_event(&in_block(
        W,
        Vec::new(),
        vec![record(&t1, TransactionContext::InBlock(block(10)))],
    ));
    assert_eq!(status(a1), Some(DispatchState::Sent), "confirmed now");

    // First seen in a block.
    let a2 = maybe_out(&l, &t2).await;
    hub.apply_wallet_event(&in_block(
        W,
        vec![record(&t2, TransactionContext::InBlock(block(11)))],
        Vec::new(),
    ));
    assert_eq!(status(a2), Some(DispatchState::Sent));

    // An InstantSend lock on a mempool transaction.
    let a3 = maybe_out(&l, &t3).await;
    hub.apply_wallet_event(&WalletEvent::TransactionInstantLocked {
        wallet_id: W.0,
        txid: t3.txid(),
        instant_lock: InstantLock::default(),
        balance: Default::default(),
        account_balances: BTreeMap::new(),
    });
    assert_eq!(status(a3), Some(DispatchState::Sent));

    // First seen through its InstantSend lock.
    let a4 = maybe_out(&l, &t4).await;
    hub.apply_wallet_event(&detected(
        W,
        record(&t4, TransactionContext::InstantSend(InstantLock::default())),
    ));
    assert_eq!(status(a4), Some(DispatchState::Sent));

    // A ChainLock over the block that holds it.
    let a5 = maybe_out(&l, &t5).await;
    hub.apply_wallet_event(&WalletEvent::ChainLockProcessed {
        wallet_id: W.0,
        chain_lock: ChainLock {
            block_height: 12,
            block_hash: BlockHash::all_zeros(),
            signature: BLSSignature::from([0; 96]),
        },
        locked_transactions: BTreeMap::from([(
            AccountType::Standard {
                index: 0,
                standard_account_type: StandardAccountType::BIP44Account,
            },
            vec![t5.txid()],
        )]),
    });
    assert_eq!(status(a5), Some(DispatchState::Sent));

    assert_eq!(
        resolved(&rec),
        [a1, a2, a3, a4, a5].map(|a| ev(W, a, Sent)).to_vec(),
        "one DispatchResolved(Sent) each"
    );
}

/// After a restart a step-marked artifact has no row-less entry (row-less
/// state is not persisted) and reads `MaybeSent` by its marker alone. The
/// wallet's sighting settles it: `Sent`, announced, its `Sent` row written
/// with the marker (DEC-163). Another wallet's sighting does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sighting_settles_an_artifact_known_by_its_marker_alone() {
    use super::fence::JournalBackend;
    let a = tests::art(9);
    let j = Arc::new(tests::FakeJournal::default());
    j.insert_step(&W.0, "dashpay/contact/1/send", &a.0).unwrap();
    let (t, rec) = table();
    let (rows, steps) = j.load();
    t.load_journal(Some(j.clone() as Arc<dyn JournalBackend>), rows, steps);
    assert_eq!(t.dispatch_status(W, a), Some(DispatchState::MaybeSent));
    t.note_seen(W2, a);
    assert_eq!(t.dispatch_status(W, a), Some(DispatchState::MaybeSent));
    t.note_seen(W, a);
    assert_eq!(t.dispatch_status(W, a), Some(DispatchState::Sent));
    assert_eq!(resolved(&rec), vec![ev(W, a, Sent)]);
    tests::until(|| j.load().1.iter().any(|r| r.sent)).await;
}
