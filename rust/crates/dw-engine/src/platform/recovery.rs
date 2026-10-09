//! Same-seed recovery and the main identity (DASHPAY §3.2, §3.4; ROADMAP
//! DP1-05): the identity read model behind `DashPay::identities`, the main
//! identity and each identity's main name, the names pass that follows a
//! discovery, and `discover_identities`.
//!
//! A restore recovers in two passes:
//!
//! 1. the bring-up (`bringup.rs`): `start_wallet_subsystems` discovers the
//!    seed's identities, fetches their names as far as its budget allows,
//!    runs a DashPay pass and turns the contact requests into DIP-15
//!    accounts, all before SPV starts, so SPV's first filter scan finds the
//!    contact payments;
//! 2. the names pass, right after it: a full DPNS refresh of each wallet
//!    whose identities that bring-up discovered. The library's enrichment
//!    is best-effort and stops at the budget; the `dpns_sync` loop would
//!    pick the rest up, but only every 10 minutes for a wallet added while
//!    it runs. At a start the loop's first pass does the same refresh at
//!    once; ours waits for it (the two share the library's DPNS gate) and
//!    repeats it, once per recovery, which buys a pass that ends inside
//!    this module's bounds rather than whenever the loop's does. A pass cut
//!    off by a stop is owed again at the next start.
//!
//! The main identity is ours (§1): the `dp_main_identity` row while that
//! identity is still the wallet's, else the lowest identity index. The main
//! name is DP1-03's rule ([`resolve_main_name`]): the `dp_prefs` `main_name`
//! row while the identity still owns that label, else its temporary or won
//! contested name, else the name acquired first, over the names Platform
//! evidence shows it owns ([`evident_names`]: a label whose write may be in
//! flight is none). The rows are wallet rows, so a `.dwbackup` brings them
//! back; a restore from the phrase alone gets the defaults, which are what a
//! fresh choice would show on every machine.
//!
//! The choice API other tasks build on (DP1-03's main-name choice and
//! contest outcomes; stable from DP1-05):
//!
//! - `NetworkSession::set_main_name(wallet, identity, Some(label) | None)`
//!   stores (or clears, back to the default) an identity's main name in
//!   `dp_prefs` under [`MAIN_NAME_PREF`]; `set_name_pref` does the same for
//!   DP1-03's other main-name rows ([`MainNamePrefs`]), and `name_prefs`
//!   reads them; `set_main_identity_of` is `DashPay::set_main_identity`'s
//!   body and updates the cache. All serialize with each other and with
//!   loads, so the cache ends as the database does. Write the rows only
//!   through them, never directly.
//! - Which names show is never the cache's to say (DEC-124): `identities()`
//!   reads the main-name rows from the database on every call, in one read,
//!   and filters the library's list, or the last snapshot of it, with them.
//! - [`resolve_main_name`] is the selection rule; `identities()` and
//!   `DashPay::main_name` apply it.
//! - dw-appdb's `main_identity`/`set_main_identity` and
//!   `dp_prefs`/`set_dp_pref` are the storage underneath.
//!
//! `identities()` is a sync call (m4 §1): it reads the library's in-memory
//! identity state when its lock is free and the last snapshot otherwise, the
//! main identity from a cache loaded at open, at each bring-up and on writes,
//! and the main-name rows from the database.

use std::collections::{BTreeMap, HashMap, HashSet};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dpp::identity::accessors::IdentityGettersV0;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::util::strings::convert_to_homograph_safe_chars;
use dw_vault::{GrantKind, KeyHold, ScanKey, Vault, VaultError};
use platform_wallet::changeset::{DpnsNameSaleStatus, DpnsNameStateEntry};
use platform_wallet::manager::startup::DEFAULT_STARTUP_BUDGET;
use platform_wallet::wallet::identity::network::IdentityDiscoveryOptions;
use platform_wallet::{DpnsNameInfo, PlatformWalletInfo};
use tokio::sync::watch;

use super::VaultScanKey;
use super::bringup::until;
use super::errors::{IdentityError, PlatformError};
use super::identity::IdentitySummary;
use super::keys_policy::missing_dashpay_purposes;
use super::names::{MainNamePrefs, evident_names, resolve_main_name, same_name, shown_names};
use super::profile::Profile;
use super::runtime::PlatformSignal;
use super::runtime::guard;
use crate::session::Manager;
use crate::{EngineError, NetworkSession, WalletId};

/// The `dp_prefs` key of an identity's chosen main name (DP1-03 writes it
/// when the user picks one).
pub(crate) const MAIN_NAME_PREF: &str = "main_name";
/// The longest a names pass may take: a DPNS walk is a few queries per
/// identity, so this is generous; a pass cut off is finished by `dpns_sync`.
const NAMES_PASS_BUDGET: Duration = DEFAULT_STARTUP_BUDGET;
/// How long `discover_identities` may scan: a gap-limit walk is one query
/// per index, and the call holds the session open meanwhile.
const DISCOVERY_BUDGET: Duration = Duration::from_secs(60);

/// One of the wallet's identities as the library holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OwnedIdentity {
    pub(super) identity: String,
    pub(super) index: u32,
    /// The owned DPNS labels in the library's order, each with when it was
    /// acquired (ms), if known.
    pub(super) names: Vec<(String, Option<u64>)>,
    /// The labels it contends for (DP1-03): not names yet.
    pub(super) open_contests: Vec<String>,
    /// The labels (normalized) a marketplace row says it owns.
    pub(super) row_owned: Vec<String>,
    pub(super) balance: Option<u64>,
    pub(super) has_dashpay_keys: bool,
    pub(super) profile: Option<Profile>,
}

/// The wallet's identity choices, as stored: the main identity (cached) and
/// identity → its main-name rows (read from the database for each call).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct IdentityChoices {
    pub(super) main_identity: Option<String>,
    pub(super) names: HashMap<String, MainNamePrefs>,
}

/// The session's recovery state: the choices cache, the last identity
/// snapshot per wallet, the wallets that owe a names pass, and the
/// explicit discoveries under way.
#[derive(Default)]
pub(crate) struct Recovery {
    choices: Mutex<HashMap<WalletId, IdentityChoices>>,
    /// Held across each choice's database write or read and its cache
    /// update, so the cache ends as the database does (review r1 N2).
    choice_writer: tokio::sync::Mutex<()>,
    snapshots: Mutex<HashMap<WalletId, Vec<OwnedIdentity>>>,
    names_due: Mutex<HashSet<WalletId>>,
    /// Per wallet, the admission of its explicit discoveries (review r1 M3,
    /// r2 M3-R2).
    discoveries: Mutex<HashMap<WalletId, DiscoveryGate>>,
    /// The last wallet generation a [`DiscoveryGate`] was opened for.
    generations: AtomicU64,
    /// Names passes started this session.
    #[cfg(test)]
    pub(super) names_passes: AtomicU32,
    #[cfg(test)]
    pub(super) mock: Mutex<Option<Arc<dyn MockPlatform>>>,
    /// Tests: holds the next `set_main_identity` between its database write
    /// and its cache update.
    #[cfg(test)]
    pub(super) pause_after_choice: Mutex<Option<Arc<Pause>>>,
}

/// A wallet's explicit discoveries: the channel that ends them, the wallet
/// generation they belong to, and how many teardowns (removal, unload, a
/// restore's rollback) hold admission closed. The gate lives from the first
/// discovery until the last teardown holding it is done; the next one opens
/// a new generation, so nothing admitted before a teardown applies after it.
struct DiscoveryGate {
    ending: Arc<watch::Sender<bool>>,
    generation: u64,
    closers: usize,
}

/// Holds a wallet's discovery admission closed while its teardown runs;
/// dropping it, on any path out of the teardown, reopens it for a new
/// generation once no other teardown holds it.
pub(crate) struct DiscoveriesClosed<'a> {
    recovery: &'a Recovery,
    id: WalletId,
}

impl Drop for DiscoveriesClosed<'_> {
    fn drop(&mut self) {
        let mut all = guard(&self.recovery.discoveries);
        if let Some(gate) = all.get_mut(&self.id) {
            gate.closers -= 1;
            if gate.closers == 0 {
                all.remove(&self.id);
            }
        }
    }
}

/// A test's hold on one call at a known point.
#[cfg(test)]
#[derive(Default)]
pub(super) struct Pause {
    pub(super) reached: AtomicBool,
    pub(super) release: tokio::sync::Notify,
}

impl Recovery {
    fn choices_of(&self, id: &WalletId) -> IdentityChoices {
        guard(&self.choices).get(id).cloned().unwrap_or_default()
    }

    /// Applies a choice just written to the database, under the writer
    /// lock that write took.
    fn update_choices(&self, id: WalletId, update: impl FnOnce(&mut IdentityChoices)) {
        update(guard(&self.choices).entry(id).or_default());
    }

    fn gate<'a>(
        &self,
        all: &'a mut HashMap<WalletId, DiscoveryGate>,
        id: WalletId,
    ) -> &'a mut DiscoveryGate {
        all.entry(id).or_insert_with(|| DiscoveryGate {
            ending: Arc::new(watch::channel(false).0),
            generation: self.generations.fetch_add(1, Ordering::SeqCst) + 1,
            closers: 0,
        })
    }

    /// Admits an explicit discovery of `id`: its wallet generation, and the
    /// receiver that turns `true` when that generation is torn down. `None`
    /// while a teardown holds admission closed.
    fn begin_discovery(&self, id: WalletId) -> Option<(watch::Receiver<bool>, u64)> {
        let mut all = guard(&self.discoveries);
        let gate = self.gate(&mut all, id);
        (gate.closers == 0).then(|| (gate.ending.subscribe(), gate.generation))
    }

    /// Whether a discovery of `generation` may still apply what it found:
    /// no teardown of the wallet has begun since it was admitted.
    fn admits(&self, id: WalletId, generation: u64) -> bool {
        guard(&self.discoveries)
            .get(&id)
            .is_some_and(|gate| gate.generation == generation && gate.closers == 0)
    }

    /// Closes the discovery admission of a wallet about to be torn down,
    /// ends its explicit discoveries and waits until every one has returned,
    /// its keys dropped and nothing of it applied. Admission stays closed
    /// until the returned guard drops: hold it until the teardown is done
    /// (the wallet out of the manager, its records and secret gone).
    pub(crate) async fn end_discoveries(&self, id: WalletId) -> DiscoveriesClosed<'_> {
        let ending = {
            let mut all = guard(&self.discoveries);
            let gate = self.gate(&mut all, id);
            gate.closers += 1;
            Arc::clone(&gate.ending)
        };
        // Before the wait, so a cancelled teardown reopens admission too.
        let closed = DiscoveriesClosed { recovery: self, id };
        ending.send_replace(true);
        ending.closed().await;
        closed
    }

    /// Identities were discovered for `id`: a names pass follows (again, if
    /// the one taken was cut off).
    pub(super) fn names_due(&self, id: WalletId) {
        guard(&self.names_due).insert(id);
    }

    pub(super) fn take_names_due(&self) -> Vec<WalletId> {
        guard(&self.names_due).drain().collect()
    }

    /// Forgets a removed or closed wallet.
    pub(crate) fn forget(&self, id: &WalletId) {
        guard(&self.choices).remove(id);
        guard(&self.snapshots).remove(id);
        guard(&self.names_due).remove(id);
    }

    #[cfg(test)]
    pub(super) fn mock(&self) -> Option<Arc<dyn MockPlatform>> {
        guard(&self.mock).clone()
    }
}

/// Stands in for the library calls of a recovery in tests, so pass
/// counts and selection can be checked without Platform.
#[cfg(test)]
pub(super) trait MockPlatform: Send + Sync {
    /// `start_wallet_subsystems`.
    fn bring_up(
        &self,
        manager: Arc<Manager>,
        id: WalletId,
    ) -> BoxedFuture<Result<platform_wallet::manager::startup::WalletStartupOutcome, EngineError>>;
    /// The names pass.
    fn names_pass(&self, manager: Arc<Manager>, id: WalletId) -> BoxedFuture<()>;
    /// `discover_from_master`: the number of identities found.
    fn discover(
        &self,
        manager: Arc<Manager>,
        id: WalletId,
    ) -> BoxedFuture<Result<usize, PlatformError>>;
}

#[cfg(test)]
pub(super) type BoxedFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

/// The read model: identities by index, the main identity marked, each with
/// the names Platform evidence shows it owns ([`evident_names`]) and its main
/// name. `current`: `owned` was read from the library now, not an older
/// snapshot, whose open contests may predate a contest the identity joined
/// since (so the label it contends for shows only with a marketplace row).
pub(super) fn summaries(
    mut owned: Vec<OwnedIdentity>,
    choices: &IdentityChoices,
    current: bool,
) -> Vec<IdentitySummary> {
    owned.sort_by(|a, b| (a.index, &a.identity).cmp(&(b.index, &b.identity)));
    let main = choices
        .main_identity
        .as_deref()
        .and_then(|chosen| owned.iter().position(|o| o.identity == chosen))
        .unwrap_or(0);
    owned
        .into_iter()
        .enumerate()
        .map(|(i, o)| {
            let prefs = choices.names.get(&o.identity).cloned().unwrap_or_default();
            let row_owned = |label: &str| {
                o.row_owned
                    .contains(&convert_to_homograph_safe_chars(label))
            };
            let mut names = evident_names(o.names, &prefs, row_owned);
            if !current && let Some(contested) = &prefs.contested {
                names.retain(|(label, _)| !same_name(label, contested) || row_owned(label));
            }
            IdentitySummary {
                main_name: resolve_main_name(&names, &o.open_contests, &prefs),
                is_main: i == main,
                names: shown_names(&names, &o.open_contests),
                identity: o.identity,
                index: o.index,
                balance: o.balance,
                has_dashpay_keys: o.has_dashpay_keys,
                profile: o.profile,
                unverified: false,
            }
        })
        .collect()
}

/// An identity's names from the library's list and the marketplace sweep's
/// rows (`rows`). A name whose row says it was sold or transferred is gone,
/// though the list keeps it until the library has settled the departure.
/// Each is stamped with when Platform says the identity got it (the domain
/// document's `$transferredAt`, else `$createdAt`), and with the library's
/// stamp only without a row: that is the fetch time, the same for every
/// name a restore finds, which would leave the default main name to DPNS
/// query order.
pub(super) fn owned_names(
    identity: Identifier,
    names: &[DpnsNameInfo],
    rows: &BTreeMap<Identifier, DpnsNameStateEntry>,
) -> Vec<(String, Option<u64>)> {
    names
        .iter()
        .filter_map(|n| {
            let normalized = convert_to_homograph_safe_chars(&n.label);
            // Unique per label: DPNS keys a name's document by it, and keeps
            // that document across transfers.
            let row = rows.values().find(|row| {
                row.wallet_identity_id == identity && row.normalized_label == normalized
            });
            let acquired = match row {
                Some(row) if row.status != DpnsNameSaleStatus::Owned => return None,
                Some(row) => row.transferred_at_ms.or(row.created_at_ms),
                None => None,
            };
            Some((n.label.clone(), acquired.or(n.acquired_at)))
        })
        .collect()
}

/// The labels (normalized) of `identity` that a marketplace row says it
/// owns: Platform's evidence for a label the library listed after a write.
pub(super) fn row_owned(
    identity: Identifier,
    rows: &BTreeMap<Identifier, DpnsNameStateEntry>,
) -> Vec<String> {
    rows.values()
        .filter(|row| row.wallet_identity_id == identity && row.status == DpnsNameSaleStatus::Owned)
        .map(|row| row.normalized_label.clone())
        .collect()
}

/// The wallet's identities from the library's state.
fn owned_identities(info: &PlatformWalletInfo, id: &WalletId) -> Vec<OwnedIdentity> {
    info.identity_manager
        .wallet_managed_identities(&id.0)
        .filter_map(|m| {
            Some(OwnedIdentity {
                identity: m.identity.id().to_string(Encoding::Base58),
                // Every identity in a wallet's bucket has one.
                index: m.identity_index?,
                names: owned_names(m.identity.id(), &m.dpns_names, &info.dpns_name_states),
                open_contests: m.contested_dpns_names.clone(),
                row_owned: row_owned(m.identity.id(), &info.dpns_name_states),
                balance: Some(m.identity.balance()),
                has_dashpay_keys: missing_dashpay_purposes(m.identity.public_keys().values())
                    .is_empty(),
                profile: m.dashpay().profile.as_ref().map(|p| Profile {
                    display_name: p.display_name.clone(),
                    public_message: p.public_message.clone().or_else(|| p.bio.clone()),
                    avatar_url: p.avatar_url.clone(),
                    avatar_hash: p.avatar_hash.map(hex::encode),
                    avatar_fingerprint: p.avatar_fingerprint.map(hex::encode),
                    updated_at: None,
                }),
            })
        })
        .collect()
}

impl NetworkSession {
    /// `DashPay::identities`: in memory, but for one database read of the
    /// main-name rows, which decide the names shown (DEC-124); without that
    /// read the call fails rather than show a name it cannot vouch for.
    pub(super) fn identity_summaries(
        &self,
        id: WalletId,
    ) -> Result<Vec<IdentitySummary>, EngineError> {
        let _op = self.try_enter()?;
        self.require_wallet(&id)?;
        let manager = self.manager()?;
        let wm = manager.wallet_manager_arc();
        let recovery = &self.platform.recovery;
        // Before the identity state: a name settled or refused after this
        // read is at worst hidden by it, never shown.
        let names = wallet_name_prefs(&self.live()?.appdb, id)?;
        let (owned, current) = match wm.try_read() {
            Ok(wm) => {
                let owned = wm
                    .get_wallet_info(&id.0)
                    .map(|info| owned_identities(info, &id))
                    .unwrap_or_default();
                drop(wm);
                guard(&recovery.snapshots).insert(id, owned.clone());
                (owned, true)
            }
            // A writer holds the lock (a sync pass applying its results):
            // the last snapshot, which that write is about to replace.
            Err(_) => {
                let snapshot = guard(&recovery.snapshots).get(&id).cloned();
                (snapshot.unwrap_or_default(), false)
            }
        };
        let choices = IdentityChoices {
            names,
            ..recovery.choices_of(&id)
        };
        Ok(summaries(owned, &choices, current))
    }

    /// Takes the identity snapshot after a pass of ours changed the
    /// identities, so a read while the next writer holds the lock shows them.
    pub(super) async fn refresh_identities(&self, manager: &Manager, id: WalletId) {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let Some(owned) = wm
            .get_wallet_info(&id.0)
            .map(|info| owned_identities(info, &id))
        else {
            return;
        };
        drop(wm);
        guard(&self.platform.recovery.snapshots).insert(id, owned);
    }

    /// Loads the wallet's identity choices into the cache. A read error
    /// leaves the cache as it was: the defaults show until the next load.
    pub(crate) async fn load_identity_choices(&self, id: WalletId) {
        let recovery = &self.platform.recovery;
        let _writer = recovery.choice_writer.lock().await;
        let wallet = id.to_string();
        let read = self
            .appdb_op(move |db| {
                Ok(IdentityChoices {
                    main_identity: db.main_identity(&wallet)?,
                    names: HashMap::new(),
                })
            })
            .await;
        match read {
            Ok(read) => {
                guard(&recovery.choices).insert(id, read);
            }
            Err(e) => {
                tracing::warn!(wallet_id = %id, error = %e, "could not read the identity choices");
            }
        }
    }

    /// `DashPay::set_main_identity`.
    pub(super) async fn set_main_identity_of(
        &self,
        id: WalletId,
        identity: String,
    ) -> Result<(), PlatformError> {
        let _op = self.enter().await?;
        self.require_wallet(&id)?;
        if !self.owns_identity(id, &identity).await? {
            return Err(IdentityError::NotFound.into());
        }
        let (wallet, chosen) = (id.to_string(), identity.clone());
        let _writer = self.platform.recovery.choice_writer.lock().await;
        self.appdb_op(move |db| db.set_main_identity(&wallet, &chosen))
            .await?;
        #[cfg(test)]
        {
            let pause = guard(&self.platform.recovery.pause_after_choice).take();
            if let Some(pause) = pause {
                pause.reached.store(true, Ordering::SeqCst);
                pause.release.notified().await;
            }
        }
        self.platform
            .recovery
            .update_choices(id, |c| c.main_identity = Some(identity));
        Ok(())
    }

    /// Records `label` as `identity`'s main name; `None` returns it to the
    /// default. DP1-03's main-name choice calls it.
    pub(crate) async fn set_main_name(
        &self,
        id: WalletId,
        identity: String,
        label: Option<String>,
    ) -> Result<(), EngineError> {
        self.set_name_pref(id, identity, MAIN_NAME_PREF, label)
            .await
    }

    /// Stores one of `identity`'s main-name rows ([`MainNamePrefs::KEYS`]),
    /// or deletes it for `None`. No cache holds them (DEC-124).
    pub(crate) async fn set_name_pref(
        &self,
        id: WalletId,
        identity: String,
        key: &'static str,
        value: Option<String>,
    ) -> Result<(), EngineError> {
        let wallet = id.to_string();
        let _writer = self.platform.recovery.choice_writer.lock().await;
        self.appdb_op(move |db| db.set_dp_pref(&wallet, &identity, key, value.as_deref()))
            .await
    }

    /// `identity`'s main-name rows as stored.
    pub(crate) async fn name_prefs(
        &self,
        id: WalletId,
        identity: String,
    ) -> Result<MainNamePrefs, EngineError> {
        let _writer = self.platform.recovery.choice_writer.lock().await;
        self.appdb_op(move |db| {
            let mut all = read_name_prefs(db, id)?;
            Ok(all.remove(&identity).unwrap_or_default())
        })
        .await
    }

    async fn owns_identity(&self, id: WalletId, identity: &str) -> Result<bool, EngineError> {
        let manager = self.manager()?;
        Ok(wallet_identities(&manager, id)
            .await
            .iter()
            .any(|i| i.to_string(Encoding::Base58) == identity))
    }

    /// The names pass of a recovery (pass 2): a full DPNS refresh of the
    /// wallet's identities, within [`NAMES_PASS_BUDGET`]. Failures are
    /// logged; `dpns_sync` repairs them on its next pass.
    pub(super) async fn names_pass(&self, manager: &Arc<Manager>, id: WalletId) {
        self.run_names_pass(manager, id).await;
        self.refresh_identities(manager, id).await;
    }

    async fn run_names_pass(&self, manager: &Arc<Manager>, id: WalletId) {
        #[cfg(test)]
        {
            self.platform
                .recovery
                .names_passes
                .fetch_add(1, Ordering::SeqCst);
            if let Some(mock) = self.platform.recovery.mock() {
                return mock.names_pass(Arc::clone(manager), id).await;
            }
        }
        let Some(wallet) = manager.get_wallet(&id.0).await else {
            return;
        };
        match tokio::time::timeout(NAMES_PASS_BUDGET, wallet.identity().sync_dpns_marketplace())
            .await
        {
            Ok(Ok(summary)) => {
                tracing::info!(wallet_id = %id, ?summary, "recovery names pass done");
            }
            Ok(Err(e)) => tracing::warn!(wallet_id = %id, error = %e, "recovery names pass failed"),
            Err(_) => tracing::warn!(wallet_id = %id, "recovery names pass ran out of time"),
        }
    }

    /// `DashPay::discover_identities`: same-seed discovery past the wallet's
    /// highest known identity index, under an `IdentityScan` grant; the
    /// number of identities it stored. Forgets the proven-absence marker
    /// first, so the next start's bring-up runs discovery too even if this
    /// one finds nothing (E0-05 r1 ruling: DP6-01's "find").
    ///
    /// The key work follows the bring-up's rules: counted by `key_work`, and
    /// dropped (`Cancelled`) if the vault locks meanwhile. The wallet's
    /// removal or unload ends the call too, and waits for it
    /// ([`Recovery::end_discoveries`]). Identities the scan stored get the
    /// rest of a recovery from the supervisor, whatever the call returns: a
    /// bring-up of the wallet (its DashPay pass and contact accounts, now
    /// with an identity on file) and the names pass after it; while SPV is
    /// stopped, both come with the next start, and what a locked vault keeps
    /// the bring-up from doing, with the unlock.
    pub(super) async fn discover_identities_of(
        self: &Arc<Self>,
        id: WalletId,
        grant: String,
    ) -> Result<u32, PlatformError> {
        let _op = self.enter().await?;
        // Admitted before the wallet is checked, so a removal from here on
        // waits for this call.
        let (mut ending, generation) = self
            .platform
            .recovery
            .begin_discovery(id)
            .ok_or(PlatformError::WalletNotFound)?;
        self.require_wallet(&id)?;
        let manager = self.manager()?;
        let before = wallet_identities(&manager, id).await;
        tokio::select! {
            biased;
            // The wallet is leaving: nothing of this call applies.
            () = until(&mut ending, |ending| *ending) => Err(PlatformError::Cancelled),
            ended = self.discover(&manager, id, grant) => {
                // Bound to the generation it was admitted for.
                if !self.platform.recovery.admits(id, generation) {
                    return Err(PlatformError::Cancelled);
                }
                let stored = self.queue_discovered(&manager, id, &before).await;
                match ended {
                    // Over budget, perhaps in the names enrichment after it
                    // stored what it found: that counts.
                    Err(PlatformError::Timeout) if stored > 0 => Ok(stored),
                    ended => ended,
                }
            }
        }
    }

    /// The scan of `discover_identities_of`: the number of identities found.
    async fn discover(
        self: &Arc<Self>,
        manager: &Arc<Manager>,
        id: WalletId,
        grant: String,
    ) -> Result<u32, PlatformError> {
        let locks = self.platform.lock.borrow().locks;
        // A refused grant is the caller's answer, not failed key work.
        let (scan, _hold) = self
            .platform
            .key_work
            .run(self.vault.clone(), move |vault| {
                Ok(scan_key_for(vault, id, &grant))
            })
            .await
            .ok_or(PlatformError::Cancelled)??;
        let scan = VaultScanKey::new(scan);
        self.forget_proven_absence(id).await?;
        let wallet = manager
            .get_wallet(&id.0)
            .await
            .ok_or(PlatformError::WalletNotFound)?;
        let master = scan.resolve().map_err(|e| {
            tracing::warn!(wallet_id = %id, error = %e, "no scan key for discovery");
            PlatformError::SignerUnavailable
        })?;
        let discovery = async {
            #[cfg(test)]
            if let Some(mock) = self.platform.recovery.mock() {
                return Ok(mock.discover(Arc::clone(manager), id).await);
            }
            let found = wallet
                .identity()
                .discover_from_master(IdentityDiscoveryOptions::default(), &master);
            tokio::time::timeout(DISCOVERY_BUDGET, found)
                .await
                .map(|found| found.map(|f| f.len()).map_err(PlatformError::from))
        };
        let mut lock = self.platform.lock.subscribe();
        // The master key, the scan key and the hold drop on every return,
        // in that order.
        let found = tokio::select! {
            biased;
            () = until(&mut lock, |v| v.locks != locks) => return Err(PlatformError::Cancelled),
            ended = discovery => ended.map_err(|_| PlatformError::Timeout)??,
        };
        Ok(u32::try_from(found).unwrap_or(u32::MAX))
    }

    /// Gives the identities stored since `before` the rest of a recovery
    /// (review r1 M2: a lock or an error after the library stored them must
    /// not lose it); how many there are.
    async fn queue_discovered(
        &self,
        manager: &Manager,
        id: WalletId,
        before: &[Identifier],
    ) -> u32 {
        let stored = wallet_identities(manager, id)
            .await
            .into_iter()
            .filter(|identity| !before.contains(identity))
            .count();
        if stored > 0 {
            self.refresh_identities(manager, id).await;
            self.platform.recovery.names_due(id);
            self.platform.signal(PlatformSignal::Readmit(id));
        }
        u32::try_from(stored).unwrap_or(u32::MAX)
    }
}

/// The scan key an `IdentityScan` grant releases, with the hold it needs
/// (E0-04 §3.5): a grant authorized with the vault's key issues it directly,
/// one that carries its own key (a passphrase on a locked or mixing-only
/// vault) only through a hold, which must outlive every use of the key.
fn scan_key_for(
    vault: &Vault,
    id: WalletId,
    grant: &str,
) -> Result<(ScanKey, Option<KeyHold>), VaultError> {
    let mut tokens = [vault.redeem_grant(grant, GrantKind::IdentityScan, Some(&id.0))?];
    let hold = vault.hold_key(&mut tokens)?;
    let [token] = &tokens;
    let scan = match &hold {
        Some(hold) => vault.scan_key_held(&id.0, hold, token)?,
        None => vault.scan_key(&id.0, token)?,
    };
    Ok((scan, hold))
}

/// The wallet's main-name rows, identity → rows, in one read. The rows are
/// read in [`MainNamePrefs::KEYS`] order, the pending labels before the
/// refused ones: a label moves only from pending to refused, the refused row
/// written first, so a read across that move still sees it in one of them.
fn read_name_prefs(
    db: &dw_appdb::AppDb,
    id: WalletId,
) -> Result<HashMap<String, MainNamePrefs>, dw_appdb::AppDbError> {
    let wallet = id.to_string();
    let mut all = HashMap::<String, MainNamePrefs>::new();
    for key in MainNamePrefs::KEYS {
        for (identity, value) in db.dp_prefs(&wallet, key)? {
            all.entry(identity).or_default().set(key, Some(value));
        }
    }
    Ok(all)
}

/// [`read_name_prefs`] for the sync `identities()`.
fn wallet_name_prefs(
    db: &dw_appdb::AppDb,
    id: WalletId,
) -> Result<HashMap<String, MainNamePrefs>, EngineError> {
    read_name_prefs(db, id).map_err(|e| {
        tracing::warn!(wallet_id = %id, error = %e, "could not read the main-name rows");
        EngineError::from(e)
    })
}

/// The ids of the wallet's identities.
async fn wallet_identities(manager: &Manager, id: WalletId) -> Vec<Identifier> {
    let wm = manager.wallet_manager_arc();
    let wm = wm.read().await;
    wm.get_wallet_info(&id.0)
        .map(|info| info.identity_manager.wallet_identity_ids(&id.0))
        .unwrap_or_default()
}
