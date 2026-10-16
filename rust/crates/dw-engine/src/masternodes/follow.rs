//! The session's masternode state (MG-03 §8; not facade): the SPV list as
//! the facade reads it, the notes kept in app.sqlite (labels of wallet
//! masternodes, first sightings of bans, MG-03 D12), and the follower that
//! writes those sightings and sends the coalesced
//! `EngineEvent::Masternodes`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dw_appdb::{AppDb, AppDbError, GLOBAL_SCOPE};
use platform_wallet::masternode::MasternodeListSummary;
use tokio::runtime::Handle;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::aggregate::{OwnedMasternode, aggregate};
use crate::events::{EngineEvent, EventSink};
use crate::session::Manager;
use crate::{DashNetwork, EngineError, NetworkSession, WalletId};

/// How often the follower looks for a new list height.
const POLL: Duration = Duration::from_secs(5);
/// At most one event a second; the trailing edge is kept (MG-02's rule).
const EVENT_INTERVAL: Duration = Duration::from_secs(1);

const LABEL: &str = "masternodes.label:";
const BANNED_SEEN: &str = "masternodes.banned_seen:";
const LISTED_SEEN: &str = "masternodes.listed_seen:";

/// The SPV list as one read saw it. `entries` is empty unless `synced`.
#[derive(Debug, Clone, Default)]
pub(crate) struct ListView {
    pub synced: bool,
    /// The masternode phase's height (MG-03 D3).
    pub height: Option<u32>,
    pub entries: Vec<MasternodeListSummary>,
}

impl ListView {
    pub fn entry(&self, pro_tx_hash: &[u8; 32]) -> Option<&MasternodeListSummary> {
        self.entries.iter().find(|e| &e.pro_tx_hash == pro_tx_hash)
    }
}

/// Reads the list once: only a synced list counts (MG-03 §2).
pub(crate) async fn read_list(session: &NetworkSession, manager: &Manager) -> ListView {
    let height = session.hub.tracker().masternode_list_height();
    let entries = match height {
        Some(_) => manager.spv().masternode_list_summaries().await,
        None => None,
    };
    match entries {
        Some(entries) => ListView {
            synced: true,
            height,
            entries,
        },
        None => ListView::default(),
    }
}

/// A wallet's masternodes and its derived operator keys (by index).
pub(crate) type WalletMasternodes = (Vec<OwnedMasternode>, HashMap<[u8; 48], u32>);

/// The wallet's masternodes, aggregated from its provider transactions.
/// Blocking. `None` for a wallet the library doesn't manage.
pub(crate) fn wallet_masternodes(
    manager: &Manager,
    wallet: &WalletId,
) -> Option<WalletMasternodes> {
    let (_, txs, _, operator_index, _) = manager.provider_masternode_txs_blocking(&wallet.0)?;
    Some((aggregate(&txs), operator_index))
}

/// What app.sqlite keeps about masternodes (MG-03 D12), keyed by the
/// display-order ProTx hash.
#[derive(Debug, Clone, Default)]
pub(crate) struct Notes {
    pub labels: HashMap<String, String>,
    pub banned_seen: HashMap<String, u64>,
    pub listed_seen: HashSet<String>,
}

pub(crate) fn load_notes(db: &AppDb) -> Result<Notes, AppDbError> {
    let mut notes = Notes::default();
    for (key, value) in db.settings_with_prefix(GLOBAL_SCOPE, "masternodes.")? {
        if let Some(hash) = key.strip_prefix(LABEL) {
            notes.labels.insert(hash.into(), value);
        } else if let Some(hash) = key.strip_prefix(BANNED_SEEN) {
            if let Ok(at) = value.parse() {
                notes.banned_seen.insert(hash.into(), at);
            }
        } else if let Some(hash) = key.strip_prefix(LISTED_SEEN) {
            notes.listed_seen.insert(hash.into());
        }
    }
    Ok(notes)
}

pub(crate) fn set_label(db: &AppDb, hash: &str, label: Option<&str>) -> Result<(), AppDbError> {
    db.set_setting(GLOBAL_SCOPE, &format!("{LABEL}{hash}"), label)
}

/// A masternode's list entry as last seen (`None`: not listed).
type Seen = BTreeMap<[u8; 32], Option<MasternodeListSummary>>;

/// The follower's memory between ticks.
#[derive(Default)]
struct Followed {
    height: Option<u32>,
    generation: u64,
    seen: Seen,
}

pub(crate) struct MasternodesRuntime {
    changed: Arc<Notify>,
    /// Bumped by every change of the tracked set, a label or a key; a new
    /// value makes the follower re-read at the same height.
    generation: AtomicU64,
    coalescer: JoinHandle<()>,
    follower: Mutex<Option<JoinHandle<()>>>,
}

impl MasternodesRuntime {
    /// Built at session open on the engine runtime.
    pub fn new(network: DashNetwork, sink: Arc<dyn EventSink>, rt: &Handle) -> Self {
        let changed = Arc::new(Notify::new());
        let wake = Arc::clone(&changed);
        // A change during the pause leaves a permit, so the trailing edge is
        // sent when it ends.
        let coalescer = rt.spawn(async move {
            loop {
                wake.notified().await;
                sink.emit(EngineEvent::Masternodes {
                    network: network.clone(),
                });
                tokio::time::sleep(EVENT_INTERVAL).await;
            }
        });
        Self {
            changed,
            generation: AtomicU64::new(0),
            coalescer,
            follower: Mutex::default(),
        }
    }

    /// Something the facade changed itself (tracked set, a label, a key).
    pub fn local_change(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_one();
    }

    /// Starts following the list for the session's life.
    pub fn follow(&self, session: &Arc<NetworkSession>) {
        let weak = Arc::downgrade(session);
        let handle = session.rt.spawn(async move {
            let mut followed = Followed::default();
            loop {
                let Some(s) = weak.upgrade() else { return };
                if let Err(e) = s.masternodes.tick(&s, &mut followed).await {
                    tracing::debug!(error = %e, "masternode follower");
                }
                drop(s);
                tokio::time::sleep(POLL).await;
            }
        });
        *self.follower.lock().unwrap_or_else(|p| p.into_inner()) = Some(handle);
    }

    pub fn shutdown(&self) {
        if let Some(f) = self
            .follower
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            f.abort();
        }
        self.coalescer.abort();
    }

    /// One look at the list: on a new height (or a local change), re-reads
    /// the masternodes of interest, writes first sightings of bans and of
    /// listing (MG-00 D9), and sends the event when one of them changed.
    async fn tick(
        &self,
        session: &NetworkSession,
        followed: &mut Followed,
    ) -> Result<(), EngineError> {
        let Some(height) = session.hub.tracker().masternode_list_height() else {
            return Ok(());
        };
        let generation = self.generation.load(Ordering::SeqCst);
        if followed.height == Some(height) && followed.generation == generation {
            return Ok(());
        }
        let _op = session.try_enter()?;
        let manager = session.manager()?;
        let list = read_list(session, &manager).await;
        if !list.synced {
            return Ok(());
        }
        let wallets: Vec<WalletId> = session
            .wallet_infos()?
            .into_iter()
            .map(|w| w.wallet_id)
            .collect();
        let m = Arc::clone(&manager);
        let interest: BTreeSet<[u8; 32]> = tokio::task::spawn_blocking(move || {
            let mut set: BTreeSet<[u8; 32]> = m.tracked_masternode_hashes().into_iter().collect();
            for w in &wallets {
                if let Some((owned, _)) = wallet_masternodes(&m, w) {
                    set.extend(owned.iter().map(|o| o.record.pro_tx_hash));
                }
            }
            set
        })
        .await?;
        let seen: Seen = interest
            .iter()
            .map(|h| (*h, list.entry(h).cloned()))
            .collect();
        let appdb = session.live()?.appdb;
        let now = crate::events::unix_now();
        let entries = seen.clone();
        tokio::task::spawn_blocking(move || record_sightings(&appdb, &entries, now)).await??;
        let changed = seen != followed.seen;
        *followed = Followed {
            height: Some(height),
            generation,
            seen,
        };
        if changed {
            self.changed.notify_one();
        }
        Ok(())
    }
}

/// Writes the first sighting of each ban and of each listing, and forgets a
/// ban once the masternode is valid again (MG-00 D9).
fn record_sightings(db: &AppDb, seen: &Seen, now: u64) -> Result<(), AppDbError> {
    let notes = load_notes(db)?;
    for (hash, entry) in seen {
        let Some(entry) = entry else { continue };
        let key = super::display_hex(hash);
        if !notes.listed_seen.contains(&key) {
            db.set_setting(
                GLOBAL_SCOPE,
                &format!("{LISTED_SEEN}{key}"),
                Some(&now.to_string()),
            )?;
        }
        match (entry.is_valid, notes.banned_seen.contains_key(&key)) {
            (false, false) => db.set_setting(
                GLOBAL_SCOPE,
                &format!("{BANNED_SEEN}{key}"),
                Some(&now.to_string()),
            )?,
            (true, true) => db.set_setting(GLOBAL_SCOPE, &format!("{BANNED_SEEN}{key}"), None)?,
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(seed: u8, valid: bool) -> MasternodeListSummary {
        MasternodeListSummary {
            pro_tx_hash: [seed; 32],
            service_address: Some(format!("10.0.0.{seed}:9999").parse().unwrap()),
            platform_http_port: None,
            operator_public_key: [seed; 48],
            voting_key_id: [seed; 20],
            platform_node_id: None,
            is_valid: valid,
            is_evonode: false,
            has_extended_net_info: false,
        }
    }

    #[test]
    fn a_ban_is_noted_once_and_forgotten_when_valid_again() {
        let db = AppDb::open_in_memory().unwrap();
        let key = super::super::display_hex(&[1; 32]);
        let seen: Seen = [([1; 32], Some(summary(1, false))), ([2; 32], None)].into();
        record_sightings(&db, &seen, 100).unwrap();
        record_sightings(&db, &seen, 200).unwrap();
        let notes = load_notes(&db).unwrap();
        assert_eq!(
            notes.banned_seen.get(&key),
            Some(&100),
            "the first sighting stays"
        );
        assert!(notes.listed_seen.contains(&key));
        assert_eq!(
            notes.listed_seen.len(),
            1,
            "an unlisted masternode is not noted"
        );

        let valid: Seen = [([1; 32], Some(summary(1, true)))].into();
        record_sightings(&db, &valid, 300).unwrap();
        let notes = load_notes(&db).unwrap();
        assert!(notes.banned_seen.is_empty());
        assert!(notes.listed_seen.contains(&key));

        record_sightings(&db, &seen, 400).unwrap();
        assert_eq!(
            load_notes(&db).unwrap().banned_seen.get(&key),
            Some(&400),
            "a new ban is a new sighting"
        );
    }

    #[test]
    fn labels_round_trip_and_clear() {
        let db = AppDb::open_in_memory().unwrap();
        set_label(&db, "ab", Some("Home node")).unwrap();
        assert_eq!(
            load_notes(&db)
                .unwrap()
                .labels
                .get("ab")
                .map(String::as_str),
            Some("Home node")
        );
        set_label(&db, "ab", None).unwrap();
        assert!(load_notes(&db).unwrap().labels.is_empty());
    }
}
