//! The resolver and its cache (MG-02 D80, DEC-265).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dashcore::blockdata::transaction::special_transaction::provider_registration::{
    ProviderMasternodeType, ProviderRegistrationPayload,
};
use dashcore::bls_sig_utils::BLSPublicKey;
use dashcore::hash_types::{InputsHash, PubkeyHash};
use dashcore::hashes::Hash;
use dashcore::transaction::TransactionPayload;
use dashcore::{OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid};
use dw_appdb::AppDb;
use dw_protx::params::{EVONODE_COLLATERAL, MASTERNODE_COLLATERAL};
use platform_wallet::masternode::MasternodeListSummary;
use tokio::runtime::Handle;

use super::*;
use crate::DashNetwork;
use crate::events::{EngineEvent, EventSink};
use crate::governance::sync::GovernanceRuntime;

pub(in crate::governance) struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _: EngineEvent) {}
}

/// A ProRegTx naming `collateral` (a null txid: its own output), paying
/// `outputs`. `seed` keeps registrations with the same fields apart.
pub(in crate::governance) fn registration(
    collateral: OutPoint,
    evonode: bool,
    outputs: Vec<TxOut>,
    seed: u32,
) -> Transaction {
    let payload = ProviderRegistrationPayload {
        version: 2,
        masternode_type: if evonode {
            ProviderMasternodeType::HighPerformance
        } else {
            ProviderMasternodeType::Regular
        },
        masternode_mode: 0,
        collateral_outpoint: collateral,
        service_address: "127.0.0.1:19899".parse().unwrap(),
        owner_key_hash: PubkeyHash::from_byte_array([1; 20]),
        operator_public_key: BLSPublicKey::from([2; 48]),
        voting_key_hash: PubkeyHash::from_byte_array([3; 20]),
        operator_reward: 0,
        script_payout: ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array([4; 20])),
        inputs_hash: InputsHash::all_zeros(),
        signature: Vec::new(),
        platform_node_id: evonode.then(|| [5; 20].into()),
        platform_p2p_port: evonode.then_some(26656),
        platform_http_port: evonode.then_some(443),
    };
    Transaction {
        version: 3,
        lock_time: seed,
        input: vec![TxIn::default()],
        output: outputs,
        special_transaction_payload: Some(TransactionPayload::ProviderRegistrationPayloadType(
            payload,
        )),
    }
}

/// A regular masternode's ProRegTx with an external collateral.
pub(in crate::governance) fn external(collateral: OutPoint) -> Transaction {
    registration(collateral, false, Vec::new(), 0)
}

pub(in crate::governance) fn summary(
    pro_tx_hash: [u8; 32],
    voting_key_id: [u8; 20],
    evonode: bool,
) -> MasternodeListSummary {
    MasternodeListSummary {
        pro_tx_hash,
        service_address: None,
        platform_http_port: None,
        operator_public_key: [2; 48],
        voting_key_id,
        platform_node_id: None,
        is_valid: true,
        is_evonode: evonode,
        has_extended_net_info: false,
    }
}

pub(in crate::governance) fn snapshot(entries: Vec<MasternodeListSummary>) -> ListSnapshot {
    ListSnapshot {
        height: 1_505,
        entries,
    }
}

fn txid(tx: &Transaction) -> [u8; 32] {
    tx.txid().to_byte_array()
}

fn coins(amounts: &[u64]) -> Vec<TxOut> {
    amounts
        .iter()
        .map(|v| TxOut {
            value: *v,
            script_pubkey: ScriptBuf::new(),
        })
        .collect()
}

/// A scripted source: answers its transactions, errors for `failing`,
/// `None` otherwise, and records what it was asked.
#[derive(Default)]
pub(in crate::governance) struct Script {
    pub txs: Mutex<HashMap<[u8; 32], Transaction>>,
    pub failing: Mutex<HashSet<[u8; 32]>>,
    pub asked: Mutex<Vec<[u8; 32]>>,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}

impl Script {
    pub fn with(txs: impl IntoIterator<Item = Transaction>) -> Arc<Self> {
        let s = Self::default();
        s.txs
            .lock()
            .unwrap()
            .extend(txs.into_iter().map(|tx| (txid(&tx), tx)));
        Arc::new(s)
    }

    pub fn asked(&self) -> Vec<[u8; 32]> {
        self.asked.lock().unwrap().clone()
    }
}

impl CollateralSource for Script {
    fn registration(
        &self,
        pro_tx_hash: [u8; 32],
    ) -> BoxedFuture<'_, Result<Option<Transaction>, String>> {
        Box::pin(async move {
            self.asked.lock().unwrap().push(pro_tx_hash);
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(2)).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            if self.failing.lock().unwrap().contains(&pro_tx_hash) {
                return Err("unavailable".into());
            }
            Ok(self.txs.lock().unwrap().get(&pro_tx_hash).cloned())
        })
    }
}

/// Rounds until one asks for nothing or only fails; their sum.
pub(in crate::governance) async fn resolve_all(
    c: &Collaterals,
    list: &ListSnapshot,
    source: &dyn CollateralSource,
    appdb: &Option<Arc<AppDb>>,
) -> Round {
    let mut total = Round::default();
    loop {
        let r = resolve_round(c, list, source, appdb.clone()).await;
        total.asked += r.asked;
        total.found += r.found;
        total.failed += r.failed;
        if r.asked == r.failed {
            return total;
        }
    }
}

fn stored(db: &AppDb) -> HashMap<[u8; 32], Vec<u8>> {
    db.governance_registrations().unwrap().into_iter().collect()
}

fn op(n: u8) -> OutPoint {
    OutPoint::new(Txid::from_byte_array([n; 32]), 1)
}

#[test]
fn verify_checks_the_txid_the_payload_and_the_collateral() {
    let ext = external(op(7));
    assert_eq!(
        verify(&txid(&ext), &ext),
        Some(Collateral {
            outpoint: op(7),
            weight: 1
        })
    );
    // Another masternode's ProRegTx, under this one's hash.
    let other = external(op(8));
    assert_eq!(verify(&txid(&other), &ext), None);
    // Not a ProRegTx.
    let mut plain = ext.clone();
    plain.special_transaction_payload = None;
    assert_eq!(verify(&txid(&plain), &plain), None);
    // An internal collateral is the registration's own output, holding
    // its type's amount; an evonode weighs 4.
    let null = OutPoint::new(Txid::all_zeros(), 1);
    let evo = registration(null, true, coins(&[5, EVONODE_COLLATERAL]), 0);
    assert_eq!(
        verify(&txid(&evo), &evo),
        Some(Collateral {
            outpoint: OutPoint::new(evo.txid(), 1),
            weight: 4
        })
    );
    let short = registration(null, true, coins(&[5, MASTERNODE_COLLATERAL]), 0);
    assert_eq!(verify(&txid(&short), &short), None);
    // Bytes round-trip through app.sqlite's form.
    let bytes = dashcore::consensus::serialize(&evo);
    assert_eq!(verify_bytes(&txid(&evo), &bytes), verify(&txid(&evo), &evo));
    assert_eq!(verify_bytes(&txid(&evo), &bytes[1..]), None);
}

/// The store's outpoint takes the list's voting key and the ProRegTx's
/// type, which a revoked masternode's list entry lacks (D34).
#[test]
fn resolved_takes_the_lists_key_and_the_registrations_weight() {
    let null = OutPoint::new(Txid::all_zeros(), 0);
    let evo = registration(null, true, coins(&[EVONODE_COLLATERAL]), 0);
    let c = Collaterals::default();
    c.load(Vec::new());
    c.settle(&[(txid(&evo), verify(&txid(&evo), &evo).unwrap())], &[]);
    let mut revoked = summary(txid(&evo), [9; 20], false);
    revoked.operator_public_key = [0; 48];
    let list = snapshot(vec![revoked, summary([1; 32], [1; 20], false)]);
    assert_eq!(
        c.resolved(&list),
        HashMap::from([(
            OutPoint::new(evo.txid(), 0),
            ResolvedMasternode {
                pro_tx_hash: txid(&evo),
                voting_key_id: [9; 20],
                weight: 4,
            }
        )])
    );
}

/// DEC-265: a restart reads app.sqlite's rows and fetches nothing it
/// holds.
#[tokio::test]
async fn a_restart_fetches_nothing_already_stored() {
    let txs: Vec<Transaction> = (1..=20).map(|n| external(op(n))).collect();
    let list = snapshot(
        txs.iter()
            .map(|tx| summary(txid(tx), [1; 20], false))
            .collect(),
    );
    let source = Script::with(txs.clone());
    let db = Some(Arc::new(AppDb::open_in_memory().unwrap()));
    let first = Collaterals::default();
    let r = resolve_all(&first, &list, source.as_ref(), &db).await;
    assert_eq!((r.asked, r.found, r.failed), (20, 20, 0));
    assert_eq!(stored(db.as_ref().unwrap()).len(), 20);

    let source = Script::with([]);
    let restarted = Collaterals::default();
    let r = resolve_all(&restarted, &list, source.as_ref(), &db).await;
    assert_eq!(r, Round::default());
    assert!(source.asked().is_empty());
    assert_eq!(restarted.resolved(&list), first.resolved(&list));
    assert_eq!(restarted.resolved(&list).len(), 20);
}

/// DEC-265: a stored row that fails its check is deleted and fetched again,
/// never trusted.
#[tokio::test]
async fn a_tampered_row_is_deleted_and_fetched_again() {
    let honest = external(op(1));
    let kept = external(op(2));
    let list = snapshot(vec![
        summary(txid(&honest), [1; 20], false),
        summary(txid(&kept), [2; 20], false),
    ]);
    let db = AppDb::open_in_memory().unwrap();
    // The row of `honest` names another collateral; `kept`'s is intact.
    let tampered = external(op(3));
    db.update_governance_registrations(
        &[
            (txid(&honest), dashcore::consensus::serialize(&tampered)),
            (txid(&kept), dashcore::consensus::serialize(&kept)),
        ],
        &[],
    )
    .unwrap();
    let db = Some(Arc::new(db));
    let source = Script::with([honest.clone(), kept.clone()]);
    let c = Collaterals::default();
    let r = resolve_all(&c, &list, source.as_ref(), &db).await;
    assert_eq!((r.asked, r.found), (1, 1));
    assert_eq!(source.asked(), vec![txid(&honest)]);
    let resolved = c.resolved(&list);
    assert!(resolved.contains_key(&op(1)) && resolved.contains_key(&op(2)));
    assert!(!resolved.contains_key(&op(3)));
    assert_eq!(
        stored(db.as_ref().unwrap())[&txid(&honest)],
        dashcore::consensus::serialize(&honest)
    );

    // Undecodable bytes go the same way.
    let db2 = AppDb::open_in_memory().unwrap();
    db2.update_governance_registrations(&[(txid(&honest), vec![1, 2, 3])], &[])
        .unwrap();
    let db2 = Some(Arc::new(db2));
    let c = Collaterals::default();
    let r = resolve_all(&c, &list, source.as_ref(), &db2).await;
    assert_eq!((r.asked, r.found), (2, 2));
    assert_eq!(stored(db2.as_ref().unwrap()).len(), 2);
}

/// A fetch that failed is asked again; a ProRegTx the source doesn't have,
/// or another transaction under its hash, is asked for 3 times (one DAPI
/// node may lag) and then not again this session. Nothing unlisted is ever
/// asked for.
#[tokio::test]
async fn a_failed_fetch_is_retried_a_missing_one_three_times() {
    let a = external(op(1));
    let b = external(op(2));
    let liar = external(op(3));
    let list = snapshot(vec![
        summary(txid(&a), [1; 20], false),
        summary(txid(&b), [2; 20], false),
        summary([0xee; 32], [3; 20], false),
        summary([0xdd; 32], [4; 20], false),
    ]);
    let source = Script::with([a.clone(), b.clone()]);
    // Another transaction under 0xdd's hash.
    source.txs.lock().unwrap().insert([0xdd; 32], liar);
    source.failing.lock().unwrap().insert(txid(&b));
    let c = Collaterals::default();
    let r = resolve_all(&c, &list, source.as_ref(), &None).await;
    assert_eq!((r.asked, r.found, r.failed), (4 + 3 + 3 + 1, 1, 4));
    let times = |k: [u8; 32]| source.asked().iter().filter(|a| **a == k).count();
    assert_eq!((times([0xee; 32]), times([0xdd; 32])), (3, 3));
    // The failed one comes back, the missing and false ones never.
    let r = resolve_round(&c, &list, source.as_ref(), None).await;
    assert_eq!((r.asked, r.failed), (1, 1));
    source.failing.lock().unwrap().clear();
    let r = resolve_all(&c, &list, source.as_ref(), &None).await;
    assert_eq!((r.asked, r.found, r.failed), (1, 1, 0));
    assert_eq!(c.resolved(&list).len(), 2);
    assert_eq!((times([0xee; 32]), times([0xdd; 32])), (3, 3));
    let listed: HashSet<[u8; 32]> = list.entries.iter().map(|e| e.pro_tx_hash).collect();
    assert!(source.asked().iter().all(|k| listed.contains(k)));
    assert_eq!(c.fetches(), 13);
}

/// The follower hands the store a resolved set only when it changed:
/// `set_resolved` forgets dismissed verdicts and checks every vote again.
#[tokio::test]
async fn the_store_gets_a_resolved_set_only_when_it_changed() {
    let a = external(op(1));
    let b = external(op(2));
    let list = |key_a: u8, valid_c: bool| {
        let mut c = summary([0xcc; 32], [9; 20], false);
        c.is_valid = valid_c;
        snapshot(vec![
            summary(txid(&a), [key_a; 20], false),
            summary(txid(&b), [2; 20], false),
            c,
        ])
    };
    let source = Script::with([a.clone(), b.clone()]);
    let rt = GovernanceRuntime::new(
        DashNetwork::Regtest,
        Arc::new(NullSink),
        &Handle::current(),
        true,
        Some(source.clone()),
    );
    let sets = || rt.resolved_sets.load(Ordering::SeqCst);
    rt.apply_list(list(1, true));
    rt.apply_resolved();
    assert_eq!(sets(), 0);
    resolve_all(&rt.collaterals, &list(1, true), source.as_ref(), &None).await;
    rt.apply_resolved();
    assert_eq!(sets(), 1);
    rt.apply_resolved();
    rt.apply_list(list(1, true));
    assert_eq!(sets(), 1);
    // Another masternode's change leaves the set as it was.
    rt.apply_list(list(1, false));
    rt.apply_resolved();
    assert_eq!(sets(), 1);
    // A resolved masternode's new voting key changes it.
    rt.apply_list(list(7, false));
    assert_eq!(sets(), 2);
    rt.apply_resolved();
    assert_eq!(sets(), 2);
}

/// DEC-265: at most 8 fetches at a time, 4,000 a session; the rest stay
/// unresolved.
#[tokio::test]
async fn fetches_stop_at_the_session_ceiling_eight_at_a_time() {
    let list = snapshot(
        (0..SESSION_FETCHES + 50)
            .map(|n| {
                let mut h = [0u8; 32];
                h[..4].copy_from_slice(&n.to_le_bytes());
                summary(h, [1; 20], false)
            })
            .collect(),
    );
    let source = Script::with([]);
    let c = Collaterals::default();
    let r = resolve_all(&c, &list, source.as_ref(), &None).await;
    assert_eq!(r.asked, SESSION_FETCHES as usize);
    assert_eq!(source.asked().len(), SESSION_FETCHES as usize);
    assert_eq!(c.fetches(), SESSION_FETCHES);
    assert_eq!(
        resolve_round(&c, &list, source.as_ref(), None).await,
        Round::default()
    );
    assert_eq!(source.max_in_flight.load(Ordering::SeqCst), IN_FLIGHT);
}

/// DEC-265: a masternode the list drops leaves the cache and app.sqlite at
/// the next list update; an empty list drops nothing.
#[tokio::test]
async fn a_dropped_masternode_leaves_the_cache_and_app_sqlite() {
    let a = external(op(1));
    let b = external(op(2));
    let both = snapshot(vec![
        summary(txid(&a), [1; 20], false),
        summary(txid(&b), [2; 20], false),
    ]);
    let source = Script::with([a.clone(), b.clone()]);
    let rt = GovernanceRuntime::new(
        DashNetwork::Regtest,
        Arc::new(NullSink),
        &Handle::current(),
        true,
        Some(source.clone()),
    );
    let db = Some(Arc::new(AppDb::open_in_memory().unwrap()));
    assert!(rt.apply_list(both.clone()).is_empty());
    resolve_all(&rt.collaterals, &both, source.as_ref(), &db).await;
    assert_eq!(stored(db.as_ref().unwrap()).len(), 2);

    assert!(rt.apply_list(snapshot(Vec::new())).is_empty());
    let generation = rt.collaterals.generation();
    let rekeyed = snapshot(vec![
        summary(txid(&a), [9; 20], false),
        summary(txid(&b), [2; 20], false),
    ]);
    assert!(rt.apply_list(rekeyed).is_empty());
    assert_eq!(
        rt.collaterals.generation(),
        generation,
        "no collateral left"
    );
    let gone = rt.apply_list(snapshot(vec![summary(txid(&b), [2; 20], false)]));
    assert_eq!(gone, vec![txid(&a)]);
    assert_eq!(rt.collaterals.generation(), generation + 1);
    persist(&db, Vec::new(), gone).await;
    assert_eq!(
        stored(db.as_ref().unwrap()).into_keys().collect::<Vec<_>>(),
        vec![txid(&b)]
    );
    assert_eq!(rt.collaterals.resolved(&both).len(), 1);
}

/// Rows a later list no longer has are deleted when they are first read.
#[tokio::test]
async fn rows_of_unlisted_masternodes_are_deleted_on_reading() {
    let a = external(op(1));
    let b = external(op(2));
    let db = AppDb::open_in_memory().unwrap();
    db.update_governance_registrations(
        &[
            (txid(&a), dashcore::consensus::serialize(&a)),
            (txid(&b), dashcore::consensus::serialize(&b)),
        ],
        &[],
    )
    .unwrap();
    let db = Some(Arc::new(db));
    let list = snapshot(vec![summary(txid(&b), [2; 20], false)]);
    let source = Script::with([]);
    let c = Collaterals::default();
    assert_eq!(
        resolve_all(&c, &list, source.as_ref(), &db).await,
        Round::default()
    );
    assert_eq!(
        stored(db.as_ref().unwrap()).into_keys().collect::<Vec<_>>(),
        vec![txid(&b)]
    );
}
