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
//! name is the `dp_prefs` `main_name` row while the identity still owns that
//! label, else the name acquired first. Both rows are wallet rows, so a
//! `.dwbackup` brings them back; a restore from the phrase alone gets the
//! defaults, which are what a fresh choice would show on every machine.
//!
//! `identities()` is a sync read (m4 §1): it reads the library's in-memory
//! identity state when its lock is free and the last snapshot otherwise, and
//! the choices from a cache loaded at open, at each bring-up and on writes.

use std::collections::{BTreeMap, HashMap, HashSet};
#[cfg(test)]
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dpp::identity::accessors::IdentityGettersV0;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::util::strings::convert_to_homograph_safe_chars;
use dw_vault::GrantKind;
use platform_wallet::changeset::{DpnsNameSaleStatus, DpnsNameStateEntry};
use platform_wallet::manager::startup::DEFAULT_STARTUP_BUDGET;
use platform_wallet::wallet::identity::network::IdentityDiscoveryOptions;
use platform_wallet::{DpnsNameInfo, PlatformWalletInfo};

use super::VaultScanKey;
use super::bringup::until;
use super::errors::{IdentityError, PlatformError};
use super::identity::IdentitySummary;
use super::keys_policy::missing_dashpay_purposes;
use super::profile::Profile;
use super::runtime::PlatformSignal;
use super::runtime::guard;
use crate::session::Manager;
use crate::{EngineError, NetworkSession, WalletId};

/// The `dp_prefs` key of an identity's chosen main name (DP1-03 writes it
/// when the user picks one or a contest resolves).
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
    pub(super) balance: Option<u64>,
    pub(super) has_dashpay_keys: bool,
    pub(super) profile: Option<Profile>,
}

/// The wallet's identity choices, as stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct IdentityChoices {
    pub(super) main_identity: Option<String>,
    /// identity → its chosen main name.
    pub(super) main_names: HashMap<String, String>,
}

/// The session's recovery state: the choices cache, the last identity
/// snapshot per wallet, and the wallets that owe a names pass.
#[derive(Default)]
pub(crate) struct Recovery {
    choices: Mutex<HashMap<WalletId, IdentityChoices>>,
    /// Bumped by every choice written, so a load that read the database
    /// before the write does not put the older rows back over it.
    choice_writes: AtomicU64,
    snapshots: Mutex<HashMap<WalletId, Vec<OwnedIdentity>>>,
    names_due: Mutex<HashSet<WalletId>>,
    /// Names passes started this session.
    #[cfg(test)]
    pub(super) names_passes: AtomicU32,
    #[cfg(test)]
    pub(super) mock: Mutex<Option<Arc<dyn MockPlatform>>>,
}

impl Recovery {
    fn choices_of(&self, id: &WalletId) -> IdentityChoices {
        guard(&self.choices).get(id).cloned().unwrap_or_default()
    }

    /// Applies a choice just written to the database.
    fn update_choices(&self, id: WalletId, update: impl FnOnce(&mut IdentityChoices)) {
        let mut choices = guard(&self.choices);
        self.choice_writes.fetch_add(1, Ordering::SeqCst);
        update(choices.entry(id).or_default());
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

/// The name shown for an identity: `preferred` while the identity still
/// owns it (as DPNS compares labels: homograph-normalized), else the name
/// acquired first (unknown times last, then the library's order). `None`
/// while it owns none: a label still in a contest is not a name yet, and
/// the temporary name is an owned one (F5).
pub(super) fn main_name(
    names: &[(String, Option<u64>)],
    preferred: Option<&str>,
) -> Option<String> {
    if let Some(preferred) = preferred.map(convert_to_homograph_safe_chars)
        && let Some((label, _)) = names
            .iter()
            .find(|(label, _)| convert_to_homograph_safe_chars(label) == preferred)
    {
        return Some(label.clone());
    }
    names
        .iter()
        .enumerate()
        .min_by_key(|(i, (_, at))| (at.is_none(), *at, *i))
        .map(|(_, (label, _))| label.clone())
}

/// The read model: identities by index, the main identity marked, each with
/// its main name.
pub(super) fn summaries(
    mut owned: Vec<OwnedIdentity>,
    choices: &IdentityChoices,
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
            let preferred = choices.main_names.get(&o.identity).map(String::as_str);
            IdentitySummary {
                main_name: main_name(&o.names, preferred),
                is_main: i == main,
                names: o.names.into_iter().map(|(label, _)| label).collect(),
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
    /// `DashPay::identities`: in-memory only (m4 §1).
    pub(super) fn identity_summaries(
        &self,
        id: WalletId,
    ) -> Result<Vec<IdentitySummary>, EngineError> {
        let _op = self.try_enter()?;
        self.require_wallet(&id)?;
        let manager = self.manager()?;
        let wm = manager.wallet_manager_arc();
        let recovery = &self.platform.recovery;
        let owned = match wm.try_read() {
            Ok(wm) => {
                let owned = wm
                    .get_wallet_info(&id.0)
                    .map(|info| owned_identities(info, &id))
                    .unwrap_or_default();
                drop(wm);
                guard(&recovery.snapshots).insert(id, owned.clone());
                owned
            }
            // A writer holds the lock (a sync pass applying its results):
            // the last snapshot, which that write is about to replace.
            Err(_) => guard(&recovery.snapshots)
                .get(&id)
                .cloned()
                .unwrap_or_default(),
        };
        Ok(summaries(owned, &recovery.choices_of(&id)))
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
        let writes = recovery.choice_writes.load(Ordering::SeqCst);
        let wallet = id.to_string();
        let read = self
            .appdb_op(move |db| {
                Ok(IdentityChoices {
                    main_identity: db.main_identity(&wallet)?,
                    main_names: db.dp_prefs(&wallet, MAIN_NAME_PREF)?.into_iter().collect(),
                })
            })
            .await;
        match read {
            Ok(read) => {
                let mut choices = guard(&recovery.choices);
                // A write since the read is newer, and already applied.
                if recovery.choice_writes.load(Ordering::SeqCst) == writes {
                    choices.insert(id, read);
                }
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
        self.appdb_op(move |db| db.set_main_identity(&wallet, &chosen))
            .await?;
        self.platform
            .recovery
            .update_choices(id, |c| c.main_identity = Some(identity));
        Ok(())
    }

    /// Records `label` as `identity`'s main name; `None` returns it to the
    /// default. DP1-03's main-name choice and its contest outcomes call it.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "DP1-03's main-name choice calls it")
    )]
    pub(crate) async fn set_main_name(
        &self,
        id: WalletId,
        identity: String,
        label: Option<String>,
    ) -> Result<(), EngineError> {
        let (wallet, who, value) = (id.to_string(), identity.clone(), label.clone());
        self.appdb_op(move |db| db.set_dp_pref(&wallet, &who, MAIN_NAME_PREF, value.as_deref()))
            .await?;
        self.platform.recovery.update_choices(id, |c| {
            match label {
                Some(label) => c.main_names.insert(identity, label),
                None => c.main_names.remove(&identity),
            };
        });
        Ok(())
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
    /// dropped (`Cancelled`) if the vault locks meanwhile. What it finds gets
    /// the rest of a recovery from the supervisor rather than under this
    /// call's gate: a bring-up of the wallet (its DashPay pass and contact
    /// accounts, now with an identity on file) and the names pass after it;
    /// while SPV is stopped, both come with the next start.
    pub(super) async fn discover_identities_of(
        self: &Arc<Self>,
        id: WalletId,
        grant: String,
    ) -> Result<u32, PlatformError> {
        let _op = self.enter().await?;
        self.require_wallet(&id)?;
        let manager = self.manager()?;
        let locks = self.platform.lock.borrow().locks;
        let scan = self
            .platform
            .key_work
            .run(self.vault.clone(), move |vault| {
                // A refused grant is the caller's answer, not failed key work.
                Ok(vault
                    .redeem_grant(&grant, GrantKind::IdentityScan, Some(&id.0))
                    .and_then(|token| vault.scan_key(&id.0, &token)))
            })
            .await
            .ok_or(PlatformError::Cancelled)??;
        let scan = VaultScanKey::new(scan);
        self.forget_proven_absence(id).await?;
        let wallet = manager
            .get_wallet(&id.0)
            .await
            .ok_or(PlatformError::WalletNotFound)?;
        let before = wallet_identities(&manager, id).await.len();
        let master = scan.resolve().map_err(|e| {
            tracing::warn!(wallet_id = %id, error = %e, "no scan key for discovery");
            PlatformError::SignerUnavailable
        })?;
        let discovery = async {
            #[cfg(test)]
            if let Some(mock) = self.platform.recovery.mock() {
                return Ok(mock.discover(Arc::clone(&manager), id).await);
            }
            let found = wallet
                .identity()
                .discover_from_master(IdentityDiscoveryOptions::default(), &master);
            tokio::time::timeout(DISCOVERY_BUDGET, found)
                .await
                .map(|found| found.map(|f| f.len()).map_err(PlatformError::from))
        };
        let mut lock = self.platform.lock.subscribe();
        let ended = tokio::select! {
            biased;
            () = until(&mut lock, |v| v.locks != locks) => Err(PlatformError::Cancelled),
            ended = discovery => Ok(ended),
        };
        drop(master);
        drop(scan);
        let found = match ended? {
            Ok(found) => found?,
            // Over budget, perhaps in the names enrichment after it stored
            // what it found: that counts.
            Err(_) => match wallet_identities(&manager, id)
                .await
                .len()
                .saturating_sub(before)
            {
                0 => return Err(PlatformError::Timeout),
                stored => stored,
            },
        };
        if found > 0 {
            self.refresh_identities(&manager, id).await;
            self.platform.recovery.names_due(id);
            self.platform.signal(PlatformSignal::Readmit(id));
        }
        Ok(u32::try_from(found).unwrap_or(u32::MAX))
    }
}

/// The ids of the wallet's identities.
async fn wallet_identities(manager: &Manager, id: WalletId) -> Vec<Identifier> {
    let wm = manager.wallet_manager_arc();
    let wm = wm.read().await;
    wm.get_wallet_info(&id.0)
        .map(|info| info.identity_manager.wallet_identity_ids(&id.0))
        .unwrap_or_default()
}
