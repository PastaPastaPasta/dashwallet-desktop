//! The collateral outpoints votes name (MG-02 D80, DEC-265; not facade). A
//! vote names its masternode only by the collateral outpoint, which the
//! list lacks, so each listed masternode's ProRegTx is fetched by its
//! proTxHash. Its bytes must hash to that proTxHash, which the header-checked
//! list carries: no proof is needed, and a row of the cache in app.sqlite
//! checks itself the same way when it is read back. A failed fetch leaves the
//! masternode unresolved, as every one was before D80.
//!
//! Only proTxHashes of the list are asked for, so a fetch says nothing
//! about the wallet. Fetches run while governance sync is on, at most
//! [`IN_FLIGHT`] at a time and [`SESSION_FETCHES`] a session; the follower
//! (`sync.rs`) is the only one that hands the result to the store.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use dashcore::blockdata::transaction::special_transaction::provider_registration::ProviderMasternodeType;
use dashcore::hashes::Hash;
use dashcore::transaction::TransactionPayload;
use dashcore::{OutPoint, Transaction, Txid};
use dw_governance::params::EVONODE_VOTE_WEIGHT;
use dw_governance::store::ResolvedMasternode;
use futures_util::StreamExt;
use tokio::sync::watch;

use super::sync::ListSnapshot;
use crate::NetworkSession;

pub(super) type BoxedFuture<'a, T> = std::pin::Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Fetches at a time.
pub(super) const IN_FLIGHT: usize = 8;
/// Fetches a session makes at most; the masternodes left stay unresolved.
pub(super) const SESSION_FETCHES: u32 = 4_000;
/// Fetches between two writes to app.sqlite and the store.
const ROUND: usize = 64;
/// The pause after a round with a failed fetch; doubles up to
/// [`MAX_BACKOFF`]. A failing DAPI node is banned by the SDK, with its own
/// exponential backoff, so the next fetch goes to another one.
const MIN_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// How often an idle resolver looks for new masternodes.
const POLL: Duration = Duration::from_secs(2);
/// Answers without a usable ProRegTx (not found, undecodable, or another
/// transaction) after which a masternode is not asked for again this
/// session; one DAPI node may lag or lack the transaction.
const MISSES: u8 = 3;

/// Where ProRegTxs come from: DAPI, dashd's RPC on regtest, or a test's
/// script.
pub(crate) trait CollateralSource: Send + Sync {
    /// The transaction whose txid is `pro_tx_hash` (wire order), `None`
    /// when the source doesn't know it or sent bytes that don't decode.
    /// Unchecked. `Err` is a failed request, asked again after a pause.
    fn registration(
        &self,
        pro_tx_hash: [u8; 32],
    ) -> BoxedFuture<'_, Result<Option<Transaction>, String>>;
}

/// DAPI `getTransaction` through the SDK.
pub(crate) struct SdkSource(pub Arc<dash_sdk::Sdk>);

impl CollateralSource for SdkSource {
    fn registration(
        &self,
        pro_tx_hash: [u8; 32],
    ) -> BoxedFuture<'_, Result<Option<Transaction>, String>> {
        Box::pin(async move {
            let settings = dash_sdk::RequestSettings {
                timeout: Some(Duration::from_secs(10)),
                retries: Some(2),
                ban_failed_address: Some(true),
                ..Default::default()
            };
            let txid = Txid::from_byte_array(pro_tx_hash).to_string();
            match self.0.get_transaction_placement(&txid, settings).await {
                Ok(found) => Ok(found.map(|p| p.transaction)),
                // The reply's bytes don't decode (`decode_transaction`).
                Err(dash_sdk::Error::CoreError(_)) => Ok(None),
                Err(e) => Err(e.to_string()),
            }
        })
    }
}

/// dashd's `getrawtransaction` (txindex on), for regtest, which has no
/// DAPI. `url` may carry the RPC user and password.
pub(crate) struct RpcSource {
    client: reqwest::Client,
    url: url::Url,
    auth: Option<(String, String)>,
}

impl RpcSource {
    pub fn new(url: &str) -> Result<Self, String> {
        let mut url = url::Url::parse(url).map_err(|e| format!("collateral RPC URL: {e}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err("collateral RPC URL must be http(s)".into());
        }
        let auth = (!url.username().is_empty()).then(|| {
            (
                url.username().to_owned(),
                url.password().unwrap_or_default().to_owned(),
            )
        });
        let _ = url.set_username("");
        let _ = url.set_password(None);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { client, url, auth })
    }
}

impl CollateralSource for RpcSource {
    fn registration(
        &self,
        pro_tx_hash: [u8; 32],
    ) -> BoxedFuture<'_, Result<Option<Transaction>, String>> {
        Box::pin(async move {
            let txid = Txid::from_byte_array(pro_tx_hash).to_string();
            let body = serde_json::json!({
                "jsonrpc": "1.0", "id": "dw", "method": "getrawtransaction", "params": [txid, false],
            });
            let mut req = self.client.post(self.url.clone()).json(&body);
            if let Some((user, password)) = &self.auth {
                req = req.basic_auth(user, Some(password));
            }
            let reply: serde_json::Value = req
                .send()
                .await
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            // RPC_INVALID_ADDRESS_OR_KEY: no such transaction.
            if reply["error"]["code"].as_i64() == Some(-5) {
                return Ok(None);
            }
            let Some(hex) = reply["result"].as_str() else {
                return Err(format!("getrawtransaction: {}", reply["error"]));
            };
            Ok(hex::decode(hex)
                .ok()
                .and_then(|bytes| dashcore::consensus::deserialize(&bytes).ok()))
        })
    }
}

/// A resolved masternode's collateral and its vote weight, both fixed by
/// its ProRegTx.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Collateral {
    pub outpoint: OutPoint,
    pub weight: u32,
}

/// What `tx` says about masternode `pro_tx_hash`: `None` unless it is that
/// masternode's ProRegTx (its txid) and its collateral checks out. The
/// weight is of the registered type, which no update changes (Core
/// `CDeterministicMN::nType`); the list's entry of a revoked masternode
/// lacks it (D34).
pub(super) fn verify(pro_tx_hash: &[u8; 32], tx: &Transaction) -> Option<Collateral> {
    if tx.txid().as_byte_array() != pro_tx_hash {
        return None;
    }
    let Some(TransactionPayload::ProviderRegistrationPayloadType(p)) =
        &tx.special_transaction_payload
    else {
        return None;
    };
    let evonode = p.masternode_type == ProviderMasternodeType::HighPerformance;
    let outpoint = dw_protx::payloads::checked_collateral(tx, p, evonode)?;
    let weight = if evonode { EVONODE_VOTE_WEIGHT } else { 1 };
    Some(Collateral { outpoint, weight })
}

/// [`verify`] of stored bytes.
fn verify_bytes(pro_tx_hash: &[u8; 32], bytes: &[u8]) -> Option<Collateral> {
    let tx: Transaction = dashcore::consensus::deserialize(bytes).ok()?;
    verify(pro_tx_hash, &tx)
}

#[derive(Debug, Default)]
struct State {
    /// app.sqlite's rows were read (once a session, by the resolver).
    loaded: bool,
    known: HashMap<[u8; 32], Collateral>,
    /// Answers without a usable ProRegTx per listed masternode.
    misses: HashMap<[u8; 32], u8>,
    /// Listed masternodes [`MISSES`] answers had no usable ProRegTx for:
    /// not asked for again this session.
    refused: HashSet<[u8; 32]>,
    fetches: u32,
    ceiling_logged: bool,
    /// Bumped by every change of `known`.
    generation: u64,
}

/// The session's resolved collaterals, mirrored in app.sqlite.
#[derive(Debug, Default)]
pub(super) struct Collaterals {
    state: Mutex<State>,
}

impl Collaterals {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn generation(&self) -> u64 {
        self.state().generation
    }

    #[cfg(test)]
    pub fn fetches(&self) -> u32 {
        self.state().fetches
    }

    /// The store's resolved outpoints for `list`: its voting key, the
    /// ProRegTx's collateral and weight.
    pub fn resolved(&self, list: &ListSnapshot) -> HashMap<OutPoint, ResolvedMasternode> {
        let state = self.state();
        list.entries
            .iter()
            .filter_map(|e| {
                let c = state.known.get(&e.pro_tx_hash)?;
                let r = ResolvedMasternode {
                    pro_tx_hash: e.pro_tx_hash,
                    voting_key_id: e.voting_key_id,
                    operator_key: e.operator_public_key,
                    weight: c.weight,
                };
                Some((c.outpoint, r))
            })
            .collect()
    }

    /// Forgets the masternodes `list` no longer has and returns them, for
    /// app.sqlite. Nothing before the rows are read, or for an empty list.
    pub fn retain_listed(&self, list: &ListSnapshot) -> Vec<[u8; 32]> {
        let mut state = self.state();
        if !state.loaded || list.entries.is_empty() {
            return Vec::new();
        }
        let listed: HashSet<[u8; 32]> = list.entries.iter().map(|e| e.pro_tx_hash).collect();
        let gone: Vec<[u8; 32]> = state
            .known
            .keys()
            .filter(|k| !listed.contains(*k))
            .copied()
            .collect();
        for k in &gone {
            state.known.remove(k);
        }
        state.refused.retain(|k| listed.contains(k));
        state.misses.retain(|k, _| listed.contains(k));
        if !gone.is_empty() {
            state.generation += 1;
        }
        gone
    }

    /// Takes in app.sqlite's rows; returns the keys of those that failed
    /// [`verify`], to delete.
    fn load(&self, rows: Vec<([u8; 32], Vec<u8>)>) -> Vec<[u8; 32]> {
        let mut bad = Vec::new();
        let mut state = self.state();
        for (key, bytes) in rows {
            match verify_bytes(&key, &bytes) {
                Some(c) => {
                    state.known.insert(key, c);
                }
                None => bad.push(key),
            }
        }
        state.loaded = true;
        state.generation += 1;
        bad
    }

    /// Up to `n` listed masternodes to fetch, counted against the session's
    /// fetches.
    fn claim(&self, list: &ListSnapshot, n: usize) -> Vec<[u8; 32]> {
        let mut state = self.state();
        let wanted: Vec<[u8; 32]> = list
            .entries
            .iter()
            .map(|e| e.pro_tx_hash)
            .filter(|k| !state.known.contains_key(k) && !state.refused.contains(k))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let left = (SESSION_FETCHES - state.fetches) as usize;
        if left == 0 && !wanted.is_empty() && !state.ceiling_logged {
            state.ceiling_logged = true;
            tracing::warn!(
                fetches = state.fetches,
                unresolved = wanted.len(),
                "governance: the session's ProRegTx fetches are spent; the rest stay unresolved"
            );
        }
        let claimed: Vec<[u8; 32]> = wanted.into_iter().take(n.min(left)).collect();
        state.fetches += claimed.len() as u32;
        claimed
    }

    fn settle(&self, found: &[([u8; 32], Collateral)], missed: &[[u8; 32]]) {
        let mut state = self.state();
        for (k, c) in found {
            state.known.insert(*k, *c);
        }
        for k in missed {
            let n = state.misses.entry(*k).or_default();
            *n += 1;
            if *n >= MISSES {
                state.refused.insert(*k);
            }
        }
        if !found.is_empty() {
            state.generation += 1;
        }
    }

    fn loaded(&self) -> bool {
        self.state().loaded
    }
}

/// How one round of fetches went.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Round {
    pub asked: usize,
    pub found: usize,
    pub failed: usize,
}

/// Reads app.sqlite's rows once (deleting those that fail [`verify`] and
/// those `list` no longer has), then fetches one round of the listed
/// masternodes not resolved yet and stores what checks out.
pub(super) async fn resolve_round(
    collaterals: &Collaterals,
    list: &ListSnapshot,
    source: &dyn CollateralSource,
    appdb: Option<Arc<dw_appdb::AppDb>>,
) -> Round {
    if !collaterals.loaded() {
        let mut remove = collaterals.load(stored(&appdb).await);
        if !remove.is_empty() {
            tracing::warn!(
                rows = remove.len(),
                "governance: dropping stored ProRegTxs that fail their check"
            );
        }
        remove.extend(collaterals.retain_listed(list));
        persist(&appdb, Vec::new(), remove).await;
    }
    let claimed = collaterals.claim(list, ROUND);
    let mut round = Round {
        asked: claimed.len(),
        ..Round::default()
    };
    if claimed.is_empty() {
        return round;
    }
    let results: Vec<_> = futures_util::stream::iter(claimed)
        .map(|k| async move { (k, source.registration(k).await) })
        .buffer_unordered(IN_FLIGHT)
        .collect()
        .await;
    let (mut found, mut missed, mut put) = (Vec::new(), Vec::new(), Vec::new());
    for (k, r) in results {
        match r {
            Ok(Some(tx)) => match verify(&k, &tx) {
                Some(c) => {
                    found.push((k, c));
                    put.push((k, dashcore::consensus::serialize(&tx)));
                }
                None => {
                    tracing::info!(pro_tx_hash = %Txid::from_byte_array(k), "governance: the fetched ProRegTx fails its check");
                    missed.push(k);
                }
            },
            Ok(None) => missed.push(k),
            Err(e) => {
                tracing::debug!(error = %e, "governance: ProRegTx fetch failed");
                round.failed += 1;
            }
        }
    }
    round.found = found.len();
    persist(&appdb, put, Vec::new()).await;
    collaterals.settle(&found, &missed);
    round
}

/// app.sqlite's rows; none when unreadable.
async fn stored(appdb: &Option<Arc<dw_appdb::AppDb>>) -> Vec<([u8; 32], Vec<u8>)> {
    let Some(db) = appdb.clone() else {
        return Vec::new();
    };
    match tokio::task::spawn_blocking(move || db.governance_registrations()).await {
        Ok(Ok(rows)) => rows,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "governance: could not read stored ProRegTxs");
            Vec::new()
        }
        Err(_) => Vec::new(),
    }
}

/// Writes rows to app.sqlite; a failure only costs a fetch next session.
pub(super) async fn persist(
    appdb: &Option<Arc<dw_appdb::AppDb>>,
    put: Vec<([u8; 32], Vec<u8>)>,
    remove: Vec<[u8; 32]>,
) {
    let Some(db) = appdb.clone() else { return };
    if put.is_empty() && remove.is_empty() {
        return;
    }
    let written =
        tokio::task::spawn_blocking(move || db.update_governance_registrations(&put, &remove))
            .await;
    if let Ok(Err(e)) = written {
        tracing::warn!(error = %e, "governance: could not store ProRegTxs");
    }
}

/// The wait after a round: none while rounds have masternodes to fetch,
/// [`POLL`] when idle, and after a failed fetch [`MIN_BACKOFF`] doubling up
/// to [`MAX_BACKOFF`].
#[derive(Debug)]
struct Pace {
    backoff: Duration,
}

impl Default for Pace {
    fn default() -> Self {
        Self {
            backoff: MIN_BACKOFF,
        }
    }
}

impl Pace {
    fn after(&mut self, round: &Round) -> Option<Duration> {
        if round.failed > 0 {
            let wait = self.backoff;
            self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
            return Some(wait);
        }
        self.backoff = MIN_BACKOFF;
        (round.asked == 0).then_some(POLL)
    }
}

/// Waits `d`; `true` when stopped.
async fn pause(d: Duration, stop: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(d) => false,
        r = stop.changed() => r.is_err() || *stop.borrow(),
    }
}

/// One [`resolve_round`], or `None` when sync stops first. The dropped
/// round stores nothing: its fetches still count against the session's,
/// and its masternodes stay unresolved for the next run.
pub(super) async fn round_unless_stopped(
    collaterals: &Collaterals,
    list: &ListSnapshot,
    source: &dyn CollateralSource,
    appdb: Option<Arc<dw_appdb::AppDb>>,
    stop: &mut watch::Receiver<bool>,
) -> Option<Round> {
    let round = resolve_round(collaterals, list, source, appdb);
    tokio::pin!(round);
    loop {
        tokio::select! {
            r = &mut round => return Some(r),
            r = stop.changed() => {
                if r.is_err() || *stop.borrow() {
                    return None;
                }
            }
        }
    }
}

/// The resolver: rounds while the list has masternodes to resolve, backing
/// off after a failed fetch, until sync stops. Each round that found one
/// wakes the follower to hand the store the new outpoints.
pub(super) async fn resolve_loop(session: Weak<NetworkSession>, mut stop: watch::Receiver<bool>) {
    let mut pace = Pace::default();
    loop {
        if *stop.borrow() {
            return;
        }
        let Some(s) = session.upgrade() else { return };
        let g = &s.governance;
        let Some(source) = g.source.clone() else {
            return;
        };
        let collaterals = Arc::clone(&g.collaterals);
        let list = g.list();
        let resolved = Arc::clone(&g.resolved_changed);
        let appdb = s.live().ok().map(|l| l.appdb);
        drop(s);
        let before = collaterals.generation();
        let Some(round) =
            round_unless_stopped(&collaterals, &list, source.as_ref(), appdb, &mut stop).await
        else {
            return;
        };
        if collaterals.generation() != before {
            resolved.notify_one();
        }
        let Some(wait) = pace.after(&round) else {
            continue;
        };
        if pause(wait, &mut stop).await {
            return;
        }
    }
}

#[cfg(test)]
#[path = "collateral_tests.rs"]
pub(super) mod tests;
