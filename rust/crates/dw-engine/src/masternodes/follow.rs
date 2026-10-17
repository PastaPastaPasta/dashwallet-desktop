//! The session's masternode state (MG-03 §8; not facade): the SPV list as
//! the facade reads it, the notes kept in app.sqlite (labels of wallet
//! masternodes, first sightings of bans, MG-03 D12; attached keys' ids,
//! D24), and the follower that writes those sightings and sends the
//! coalesced `EngineEvent::Masternodes`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dw_appdb::{AppDb, AppDbError, GLOBAL_SCOPE};
use platform_wallet::masternode::MasternodeListSummary;
use tokio::runtime::Handle;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::aggregate::{OwnedMasternode, add_history, aggregate};
use super::records::AttachedKeyRole;
use crate::events::{EngineEvent, EventSink};
use crate::history::HistoryStore;
use crate::session::Manager;
use crate::{DashNetwork, EngineError, NetworkSession, WalletId};

/// How often the follower looks for a new list height.
const POLL: Duration = Duration::from_secs(5);
/// At most one event a second; the trailing edge is kept (MG-02's rule).
const EVENT_INTERVAL: Duration = Duration::from_secs(1);

const LABEL: &str = "masternodes.label:";
const BANNED_SEEN: &str = "masternodes.banned_seen:";
const LISTED_SEEN: &str = "masternodes.listed_seen:";
/// A list entry was seen at version 3; it never goes back (D29).
const SHARED_SEEN: &str = "masternodes.shared_seen:";
/// `masternodes.key:<hash>:<role>`: the hash160 of an attached key.
const KEY_ID: &str = "masternodes.key:";

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
///
/// The library reads only the transactions it holds in memory, and it
/// doesn't restore persisted ones at open (MG-03 D15), so the provider
/// transactions of the session's history, which does, are added.
pub(crate) fn wallet_masternodes(
    manager: &Manager,
    history: &HistoryStore,
    wallet: &WalletId,
) -> Option<WalletMasternodes> {
    let (_, mut txs, _, operator_index, _) = manager.provider_masternode_txs_blocking(&wallet.0)?;
    history.read(wallet, |entries| {
        add_history(
            &mut txs,
            entries.values().filter(|e| !e.abandoned).map(|e| {
                let block = e.context.block_info();
                let height = block.map_or(0, |b| b.height());
                (height, block.and_then(|b| b.position()).unwrap_or(0), &e.tx)
            }),
        )
    });
    Some((aggregate(&txs), operator_index))
}

/// The masternodes of every loaded wallet, wallet by wallet. Blocking.
pub(crate) fn all_owned(manager: &Manager, history: &HistoryStore) -> Vec<OwnedMasternode> {
    manager
        .list_wallet_ids_blocking()
        .into_iter()
        .filter_map(|id| wallet_masternodes(manager, history, &WalletId(id)))
        .flat_map(|(owned, _)| owned)
        .collect()
}

/// What app.sqlite keeps about masternodes (MG-03 D12), keyed by the
/// display-order ProTx hash.
#[derive(Debug, Clone, Default)]
pub(crate) struct Notes {
    pub labels: HashMap<String, String>,
    pub banned_seen: HashMap<String, u64>,
    pub listed_seen: HashSet<String>,
    pub shared_seen: HashSet<String>,
    /// The id of each attached key, recorded at attach (D24).
    pub key_ids: HashMap<(String, AttachedKeyRole), [u8; 20]>,
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
        } else if let Some(hash) = key.strip_prefix(SHARED_SEEN) {
            notes.shared_seen.insert(hash.into());
        } else if let Some((hash, role)) = key
            .strip_prefix(KEY_ID)
            .and_then(|rest| rest.split_once(':'))
            && let (Some(role), Some(id)) =
                (AttachedKeyRole::from_vault_name(role), parse_key_id(&value))
        {
            notes.key_ids.insert((hash.into(), role), id);
        }
    }
    Ok(notes)
}

pub(crate) fn set_label(db: &AppDb, hash: &str, label: Option<&str>) -> Result<(), AppDbError> {
    db.set_setting(GLOBAL_SCOPE, &format!("{LABEL}{hash}"), label)
}

fn key_id_setting(hash: &str, role: AttachedKeyRole) -> String {
    format!("{KEY_ID}{hash}:{}", role.vault_name())
}

fn parse_key_id(hex_id: &str) -> Option<[u8; 20]> {
    let mut id = [0u8; 20];
    hex::decode_to_slice(hex_id, &mut id).ok()?;
    Some(id)
}

pub(crate) fn key_id(
    db: &AppDb,
    hash: &str,
    role: AttachedKeyRole,
) -> Result<Option<[u8; 20]>, AppDbError> {
    Ok(db
        .setting(GLOBAL_SCOPE, &key_id_setting(hash, role))?
        .and_then(|v| parse_key_id(&v)))
}

/// Records (or with `None` forgets) the id of the key attached for `role`.
pub(crate) fn set_key_id(
    db: &AppDb,
    hash: &str,
    role: AttachedKeyRole,
    id: Option<[u8; 20]>,
) -> Result<(), AppDbError> {
    db.set_setting(
        GLOBAL_SCOPE,
        &key_id_setting(hash, role),
        id.map(hex::encode).as_deref(),
    )
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

/// What makes the follower re-read at the same height, and the event's
/// wake-up.
#[derive(Default)]
struct Changes {
    notify: Notify,
    /// Bumped by every change of the tracked set, a label or a key, and of
    /// the wallet set (D28).
    generation: AtomicU64,
}

impl Changes {
    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.notify.notify_one();
    }
}

/// The session's sink, which also hears the wallet set change: a wallet
/// created, imported, restored, loaded, unloaded or removed changes whose
/// masternodes are "mine" at an unchanged list height (D28).
struct WalletSetWatch {
    sink: Arc<dyn EventSink>,
    changes: Arc<Changes>,
}

impl EventSink for WalletSetWatch {
    fn emit(&self, event: EngineEvent) {
        let wallets = matches!(
            event,
            EngineEvent::WalletCreated { .. }
                | EngineEvent::WalletRemoved { .. }
                | EngineEvent::WalletLoadChanged { .. }
        );
        self.sink.emit(event);
        if wallets {
            self.changes.bump();
        }
    }
}

pub(crate) struct MasternodesRuntime {
    changes: Arc<Changes>,
    coalescer: JoinHandle<()>,
    follower: Mutex<Option<JoinHandle<()>>>,
    /// Held across each tracking change, so an `attach_key` can't store a
    /// key for a masternode an `untrack` is removing.
    mutations: tokio::sync::Mutex<()>,
}

impl MasternodesRuntime {
    /// Built at session open on the engine runtime.
    pub fn new(network: DashNetwork, sink: Arc<dyn EventSink>, rt: &Handle) -> Self {
        let changes = Arc::new(Changes::default());
        let wake = Arc::clone(&changes);
        // A change during the pause leaves a permit, so the trailing edge is
        // sent when it ends.
        let coalescer = rt.spawn(async move {
            loop {
                wake.notify.notified().await;
                sink.emit(EngineEvent::Masternodes {
                    network: network.clone(),
                });
                tokio::time::sleep(EVENT_INTERVAL).await;
            }
        });
        Self {
            changes,
            coalescer,
            follower: Mutex::default(),
            mutations: tokio::sync::Mutex::new(()),
        }
    }

    /// Serialises the tracking changes (see `mutations`).
    pub async fn mutation(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.mutations.lock().await
    }

    /// Something the facade changed itself (tracked set, a label, a key).
    pub fn local_change(&self) {
        self.changes.bump();
    }

    /// `sink`, also telling this runtime when the wallet set changes. The
    /// session emits through it.
    pub fn watch_wallets(&self, sink: Arc<dyn EventSink>) -> Arc<dyn EventSink> {
        Arc::new(WalletSetWatch {
            sink,
            changes: Arc::clone(&self.changes),
        })
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
}

/// A session that fails to open after the runtime was built never closes,
/// so its tasks end here.
impl Drop for MasternodesRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl MasternodesRuntime {
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
        let generation = self.changes.generation.load(Ordering::SeqCst);
        if followed.height == Some(height) && followed.generation == generation {
            return Ok(());
        }
        let _op = session.try_enter()?;
        let manager = session.manager()?;
        let list = read_list(session, &manager).await;
        if !list.synced {
            return Ok(());
        }
        let (m, hub) = (Arc::clone(&manager), Arc::clone(&session.hub));
        let interest: BTreeSet<[u8; 32]> = tokio::task::spawn_blocking(move || {
            let mut set: BTreeSet<[u8; 32]> = m.tracked_masternode_hashes().into_iter().collect();
            set.extend(
                all_owned(&m, &hub.history)
                    .iter()
                    .map(|o| o.record.pro_tx_hash),
            );
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
            self.changes.notify.notify_one();
        }
        Ok(())
    }
}

/// Writes the first sighting of each ban, of each listing and of a
/// version-3 entry (D29), and forgets a ban once the masternode is valid
/// again (MG-00 D9).
fn record_sightings(db: &AppDb, seen: &Seen, now: u64) -> Result<(), AppDbError> {
    let notes = load_notes(db)?;
    for (hash, entry) in seen {
        let Some(entry) = entry else { continue };
        let key = super::display_hex(hash);
        let note = |prefix: &str| {
            db.set_setting(
                GLOBAL_SCOPE,
                &format!("{prefix}{key}"),
                Some(&now.to_string()),
            )
        };
        if !notes.listed_seen.contains(&key) {
            note(LISTED_SEEN)?;
        }
        if entry.has_extended_net_info && !notes.shared_seen.contains(&key) {
            note(SHARED_SEEN)?;
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
        assert!(notes.shared_seen.is_empty());

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

    /// A version-3 entry is remembered, so the row stays flagged before the
    /// list syncs and after the masternode leaves it (D29).
    #[test]
    fn a_version_3_entry_is_remembered() {
        let db = AppDb::open_in_memory().unwrap();
        let key = super::super::display_hex(&[1; 32]);
        let mut shared = summary(1, true);
        shared.has_extended_net_info = true;
        record_sightings(&db, &[([1; 32], Some(shared))].into(), 100).unwrap();
        record_sightings(&db, &[([1; 32], None)].into(), 200).unwrap();
        assert!(load_notes(&db).unwrap().shared_seen.contains(&key));
    }

    /// Every wallet-set event reaches the sink and bumps the generation, so
    /// the follower re-reads at an unchanged height; others don't (D28).
    #[test]
    fn a_wallet_set_change_bumps_the_generation() {
        struct Record(Mutex<Vec<String>>);
        impl EventSink for Record {
            fn emit(&self, event: EngineEvent) {
                self.0.lock().unwrap().push(format!("{event:?}"));
            }
        }
        let sink = Arc::new(Record(Mutex::default()));
        let changes = Arc::new(Changes::default());
        let watch = WalletSetWatch {
            sink: sink.clone(),
            changes: Arc::clone(&changes),
        };
        let (network, wallet_id) = (DashNetwork::Regtest, WalletId([7; 32]));
        for event in [
            EngineEvent::WalletCreated {
                network: network.clone(),
                wallet_id,
            },
            EngineEvent::WalletLoadChanged {
                network: network.clone(),
                wallet_id,
                loaded: false,
            },
            EngineEvent::WalletRemoved { network, wallet_id },
        ] {
            watch.emit(event);
        }
        assert_eq!(changes.generation.load(Ordering::SeqCst), 3);
        watch.emit(EngineEvent::Masternodes {
            network: DashNetwork::Regtest,
        });
        assert_eq!(changes.generation.load(Ordering::SeqCst), 3);
        assert_eq!(sink.0.lock().unwrap().len(), 4, "every event is passed on");
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

    /// An attached key's id outlives the process (app.sqlite), and a read
    /// after the restart compares it with the list as it is then: a
    /// rotated voting key is a mismatch, which holds no role (D24).
    #[test]
    fn a_key_id_survives_a_restart_and_a_rotation_is_a_mismatch() {
        use super::super::records::{AttachedKey, KeyCheck};
        use super::super::tracking::KeyReferences;

        let dir = dw_testutil::private_tempdir();
        let path = dir.path().join("app.sqlite");
        let hash = super::super::display_hex(&[1; 32]);
        let attached = summary(1, true);
        {
            let db = AppDb::open(&path).unwrap();
            set_key_id(
                &db,
                &hash,
                AttachedKeyRole::Voting,
                Some(attached.voting_key_id),
            )
            .unwrap();
            set_key_id(&db, &hash, AttachedKeyRole::Owner, Some([9; 20])).unwrap();
            set_key_id(&db, &hash, AttachedKeyRole::Owner, None).unwrap();
        }
        let db = AppDb::open(&path).unwrap();
        let notes = load_notes(&db).unwrap();
        assert_eq!(notes.key_ids.len(), 1, "a forgotten id stays forgotten");
        let id = notes.key_ids[&(hash.clone(), AttachedKeyRole::Voting)];
        assert_eq!(id, attached.voting_key_id);
        assert_eq!(
            key_id(&db, &hash, AttachedKeyRole::Voting).unwrap(),
            Some(id)
        );

        let key = |entry: Option<&MasternodeListSummary>| {
            let check = KeyReferences::new(entry, None, false).check(AttachedKeyRole::Voting, id);
            AttachedKey {
                role: AttachedKeyRole::Voting,
                key_id: Some(hex::encode(id)),
                check,
            }
        };
        let same = key(Some(&attached));
        assert_eq!(same.check, KeyCheck::Matched);
        assert!(same.holds());
        // a ProUpRegTx gave the masternode another voting key
        let rotated = MasternodeListSummary {
            voting_key_id: [2; 20],
            ..attached.clone()
        };
        let stale = key(Some(&rotated));
        assert_eq!(stale.check, KeyCheck::KeyMismatch);
        assert!(!stale.holds());
        // not listed: nothing to compare with
        assert_eq!(key(None).check, KeyCheck::Unchecked);
        // no id recorded: never holds
        let unrecorded = AttachedKey {
            key_id: None,
            check: KeyCheck::Unchecked,
            ..same
        };
        assert!(!unrecorded.holds());
    }
}
